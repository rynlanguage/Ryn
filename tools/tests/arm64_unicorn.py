"""Runs a static Linux AArch64 executable under the Unicorn CPU emulator.

Only the system calls of the backend are served: write (64) to file descriptors 1 and 2, and exit (93). Every instruction
checks that sp is a multiple of 16, which Linux requires at EL0 (the hardware would raise SIGBUS otherwise). This is
emulation, not execution on an ARM processor: it shows that the instructions do what the architecture says, which is what
the backend needs; it does not measure time or check the kernel's behaviour.

The emulator is optional. `available()` is False when the `unicorn` package cannot be imported.
"""

import struct

try:
    from unicorn import Uc, UC_ARCH_ARM64, UC_MODE_ARM, UC_PROT_ALL, UC_HOOK_CODE, UC_HOOK_INTR
    from unicorn.arm64_const import UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2, UC_ARM64_REG_X8, UC_ARM64_REG_SP

    UNICORN_OK = True
except ImportError:
    UNICORN_OK = False

BASE = 0x400000
STACK_BASE = 0x10000000
STACK_SIZE = 0x800000  # 8 MiB, as the usual Linux stack limit; the program keeps 7 MiB of it
PAGE = 0x1000


class Outcome:
    def __init__(self, stdout, stderr, status, error):
        self.stdout = stdout
        self.stderr = stderr
        self.status = status
        self.error = error

    def __repr__(self):
        return f"Outcome(status={self.status}, error={self.error!r}, stdout={self.stdout!r}, stderr={self.stderr!r})"


def available():
    return UNICORN_OK


def load_segment(data):
    """The entry point and the first loadable segment of an ELF64 little-endian AArch64 executable."""
    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        raise ValueError("not an ELF64 little-endian file")
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != 183:
        raise ValueError(f"e_machine is {machine}, not AArch64 (183)")
    entry = struct.unpack_from("<Q", data, 24)[0]
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phnum = struct.unpack_from("<H", data, 56)[0]
    for index in range(phnum):
        at = phoff + index * 56
        p_type, p_flags, p_offset, p_vaddr, _, p_filesz, p_memsz, _ = struct.unpack_from("<IIQQQQQQ", data, at)
        if p_type == 1:
            return entry, p_offset, p_vaddr, p_filesz, p_memsz
    raise ValueError("no loadable segment")


def run(data, limit=5_000_000, alignment=True, timeout_us=20_000_000):
    """Runs the executable and returns an Outcome. `status` is the exit status when the program exits.

    With `alignment` every instruction is checked (slow); without it the program runs at emulator speed, and only the
    time limit stops it. The emulator is not thread-safe: call this from one thread at a time."""
    if not UNICORN_OK:
        raise RuntimeError("the unicorn package is not installed")
    entry, offset, vaddr, filesz, memsz = load_segment(data)
    emulator = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
    size = (memsz + PAGE - 1) // PAGE * PAGE
    emulator.mem_map(vaddr, size, UC_PROT_ALL)
    emulator.mem_write(vaddr, bytes(data[offset:offset + filesz]))
    emulator.mem_map(STACK_BASE, STACK_SIZE, UC_PROT_ALL)
    emulator.reg_write(UC_ARM64_REG_SP, STACK_BASE + STACK_SIZE - 16)

    stdout = bytearray()
    stderr = bytearray()
    state = {"status": None, "error": None, "count": 0}

    def code_hook(uc, address, size, user_data):
        state["count"] += 1
        sp = uc.reg_read(UC_ARM64_REG_SP)
        if sp % 16 != 0:
            state["error"] = f"sp is {sp:#x} at {address:#x}: not 16-byte aligned"
            uc.emu_stop()
        if state["count"] > limit:
            state["error"] = f"more than {limit} instructions"
            uc.emu_stop()

    def intr_hook(uc, intno, user_data):
        if intno != 2:
            state["error"] = f"exception {intno}"
            uc.emu_stop()
            return
        number = uc.reg_read(UC_ARM64_REG_X8)
        if number == 64:
            fd = uc.reg_read(UC_ARM64_REG_X0)
            buffer = uc.reg_read(UC_ARM64_REG_X1)
            length = uc.reg_read(UC_ARM64_REG_X2)
            chunk = bytes(uc.mem_read(buffer, length)) if length else b""
            if fd == 1:
                stdout.extend(chunk)
            elif fd == 2:
                stderr.extend(chunk)
            uc.reg_write(UC_ARM64_REG_X0, length)
        elif number == 93:
            state["status"] = uc.reg_read(UC_ARM64_REG_X0) & 255
            uc.emu_stop()
        else:
            state["error"] = f"system call {number} is not served"
            uc.emu_stop()

    if alignment:
        emulator.hook_add(UC_HOOK_CODE, code_hook)
    emulator.hook_add(UC_HOOK_INTR, intr_hook)
    try:
        emulator.emu_start(entry, 0, timeout=timeout_us)
    except Exception as error:  # Unicorn reports memory faults and illegal instructions as exceptions
        if state["error"] is None and state["status"] is None:
            state["error"] = f"emulation stopped: {error}"
    if state["status"] is None and state["error"] is None:
        state["error"] = "the program did not exit within the time limit"
    return Outcome(bytes(stdout), bytes(stderr), state["status"], state["error"])
