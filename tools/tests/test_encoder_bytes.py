"""The x86-64 encoder writes the bytes that llvm-mc assembles for the same instructions.

`selfhost/src/encodecheck.ryn` encodes each case with `back::x64` and prints its bytes; here the same instructions are
assembled by llvm-mc. Where the bytes differ (an equivalent form was chosen), both are disassembled and must read the same.
The cases cover EVEX (zmm 0 to 31, opmask, zeroing), the opmask VEX instructions, MMX, and YMM registers 8 to 15.
llvm-mc is required; the test is skipped without it.
"""

import shutil
import subprocess
import unittest

from ryn_harness import REPO_ROOT, ryn_binary

# The instruction of case N (1-based), in AT&T syntax as llvm-mc reads it.
CASES = [
    "vmovdqu64 4096(%rax), %zmm16",
    "vmovdqu64 %zmm17, 4096(%r9)",
    "vmovdqu64 4096(%rsp), %zmm31",
    "vmovdqu64 4096(%rax), %zmm0 {%k2}",
    "vpaddd %zmm2, %zmm1, %zmm0",
    "vpaddd %zmm16, %zmm18, %zmm20 {%k3}",
    "vpaddd %zmm16, %zmm18, %zmm20 {%k3} {z}",
    "vaddps %zmm9, %zmm8, %zmm7",
    "vpxord %zmm29, %zmm30, %zmm31",
    "vpandd %zmm3, %zmm2, %zmm1",
    "vmulps %zmm14, %zmm13, %zmm12",
    "kmovw %eax, %k1",
    "kmovw %k2, %r10d",
    "kmovq %rbx, %k7",
    "kmovq %k7, %r12",
    "kandw %k3, %k2, %k1",
    "korw %k6, %k5, %k4",
    "kxorw %k2, %k1, %k0",
    "knotw %k2, %k1",
    "kortestw %k2, %k1",
    "ktestw %k4, %k3",
    "paddd %mm0, %mm1",
    "pxor %mm3, %mm2",
    "movq %rax, %mm3",
    "movq %mm4, %r11",
    "movq 4096(%rdx), %mm5",
    "movq %mm6, 4096(%r12)",
    "emms",
    "vpaddd %ymm10, %ymm9, %ymm8",
    "vmovdqu 4096(%r9), %ymm12",
    "kmovw %r13d, %k5",
]


def llvm_mc():
    return shutil.which("llvm-mc")


def assemble(tool, lines):
    """The bytes llvm-mc writes for each instruction, read from its `# encoding:` comments."""
    text = "\n".join(lines) + "\n"
    done = subprocess.run(
        [tool, "-triple=x86_64", "-show-encoding"], input=text, capture_output=True, text=True, check=True
    )
    encodings = []
    for line in done.stdout.splitlines():
        if "encoding: [" in line:
            hexes = line.split("encoding: [", 1)[1].split("]", 1)[0]
            encodings.append(bytes(int(part, 16) for part in hexes.split(",")))
    return encodings


def disassemble(tool, encoding):
    """The text llvm-mc reads back from the bytes of one instruction."""
    text = ", ".join("0x%02x" % b for b in encoding) + "\n"
    done = subprocess.run(
        [tool, "-triple=x86_64", "-disassemble"], input=text, capture_output=True, text=True, check=True
    )
    return " ".join(line.strip() for line in done.stdout.splitlines() if line.strip())


def ryn_encodings():
    done = subprocess.run(
        [ryn_binary(), "run", str(REPO_ROOT / "selfhost/src/encodecheck.ryn")],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
    )
    if done.returncode != 0:
        raise AssertionError("encodecheck.ryn failed: " + done.stderr + done.stdout)
    found = {}
    for line in done.stdout.splitlines():
        number, *values = line.split()
        found[int(number)] = bytes(int(value) for value in values)
    return found


@unittest.skipUnless(llvm_mc(), "llvm-mc is not installed")
class EncoderBytes(unittest.TestCase):
    def test_every_case_matches_llvm_mc(self):
        tool = llvm_mc()
        ours = ryn_encodings()
        self.assertEqual(sorted(ours), list(range(1, len(CASES) + 1)), "encodecheck.ryn must print every case")
        reference = assemble(tool, CASES)
        self.assertEqual(len(reference), len(CASES), "llvm-mc must assemble every case")
        for number, expected in enumerate(reference, start=1):
            encoded = ours[number]
            if encoded == expected:
                continue
            self.assertEqual(
                disassemble(tool, encoded),
                disassemble(tool, expected),
                f"case {number} `{CASES[number - 1]}`: ryn {encoded.hex(' ')} llvm-mc {expected.hex(' ')}",
            )


if __name__ == "__main__":
    unittest.main()
