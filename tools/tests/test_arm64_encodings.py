"""The A64 encodings of the AArch64 backend (selfhost/src/back/arm64.ryn), computed from the architecture's field layout
and checked against llvm-mc. Each case: the word computed here, and the assembly text that llvm-mc must encode to it.
The test is skipped without llvm-mc."""
import shutil
import subprocess
import unittest
def enc(word): return word & 0xffffffff
def movz(rd, imm, hw): return enc(0xD2800000 | (hw << 21) | (imm << 5) | rd)
def movk(rd, imm, hw): return enc(0xF2800000 | (hw << 21) | (imm << 5) | rd)
def mov_rr(rd, rm): return enc(0xAA0003E0 | (rm << 16) | rd)          # orr rd, xzr, rm
def add_rrr(rd, rn, rm): return enc(0x8B000000 | (rm << 16) | (rn << 5) | rd)
def sub_rrr(rd, rn, rm): return enc(0xCB000000 | (rm << 16) | (rn << 5) | rd)
def mul_rrr(rd, rn, rm): return enc(0x9B007C00 | (rm << 16) | (rn << 5) | rd)
def sdiv_rrr(rd, rn, rm): return enc(0x9AC00C00 | (rm << 16) | (rn << 5) | rd)
def udiv_rrr(rd, rn, rm): return enc(0x9AC00800 | (rm << 16) | (rn << 5) | rd)
def msub_rrrr(rd, rn, rm, ra): return enc(0x9B008000 | (rm << 16) | (ra << 10) | (rn << 5) | rd)
def and_rrr(rd, rn, rm): return enc(0x8A000000 | (rm << 16) | (rn << 5) | rd)
def orr_rrr(rd, rn, rm): return enc(0xAA000000 | (rm << 16) | (rn << 5) | rd)
def eor_rrr(rd, rn, rm): return enc(0xCA000000 | (rm << 16) | (rn << 5) | rd)
def mvn_rr(rd, rm): return enc(0xAA2003E0 | (rm << 16) | rd)          # orn rd, xzr, rm
def neg_rr(rd, rm): return enc(0xCB0003E0 | (rm << 16) | rd)          # sub rd, xzr, rm
def lslv(rd, rn, rm): return enc(0x9AC02000 | (rm << 16) | (rn << 5) | rd)
def lsrv(rd, rn, rm): return enc(0x9AC02400 | (rm << 16) | (rn << 5) | rd)
def asrv(rd, rn, rm): return enc(0x9AC02800 | (rm << 16) | (rn << 5) | rd)
def cmp_rr(rn, rm): return enc(0xEB00001F | (rm << 16) | (rn << 5))  # subs xzr, rn, rm
def cset(rd, cond): return enc(0x9A9F07E0 | ((cond ^ 1) << 12) | rd)  # csinc rd, xzr, xzr, !cond
def sub_ri(rd, rn, imm): return enc(0xD1000000 | (imm << 10) | (rn << 5) | rd)
def add_ri(rd, rn, imm): return enc(0x91000000 | (imm << 10) | (rn << 5) | rd)
def str_zero(rt, rn): return enc(0xF9000000 | (rn << 5) | rt)       # str rt, [rn]
def ldr_zero(rt, rn): return enc(0xF9400000 | (rn << 5) | rt)       # ldr rt, [rn]
def str_pre16(rt): return enc(0xF81F0FE0 & ~0x1f | rt)              # str rt, [sp, #-16]!
def ldr_post16(rt): return enc(0xF84107E0 | rt)                 # ldr rt, [sp], #16
def stp_frame(): return 0xA9BF7BFD                                    # stp x29, x30, [sp, #-16]!
def ldp_frame(): return 0xA8C17BFD                                    # ldp x29, x30, [sp], #16
def mov_fp_sp(): return 0x910003FD                                    # add x29, sp, #0
def mov_sp_fp(): return 0x910003BF                                    # add sp, x29, #0
def sub_sp(imm): return enc(0xD10003FF | (imm << 10))                 # sub sp, sp, #imm
def b_(imm26): return enc(0x14000000 | (imm26 & 0x3ffffff))
def bl_(imm26): return enc(0x94000000 | (imm26 & 0x3ffffff))
def b_cond(cond, imm19): return enc(0x54000000 | ((imm19 & 0x7ffff) << 5) | cond)
def cbz(rt, imm19): return enc(0xB4000000 | ((imm19 & 0x7ffff) << 5) | rt)
def cbnz(rt, imm19): return enc(0xB5000000 | ((imm19 & 0x7ffff) << 5) | rt)
def adr(rd, imm21): return enc(0x10000000 | ((imm21 & 3) << 29) | (((imm21 >> 2) & 0x7ffff) << 5) | rd)
def ret(): return 0xD65F03C0
def svc0(): return 0xD4000001
def sbfm_sxtb(rd, rn): return enc(0x93401C00 | (rn << 5) | rd)
def sbfm_sxth(rd, rn): return enc(0x93403C00 | (rn << 5) | rd)
def sbfm_sxtw(rd, rn): return enc(0x93407C00 | (rn << 5) | rd)
def ubfm_uxtb(rd, rn): return enc(0xD3401C00 | (rn << 5) | rd)
def ubfm_uxth(rd, rn): return enc(0xD3403C00 | (rn << 5) | rd)
def ubfm_uxtw(rd, rn): return enc(0xD3407C00 | (rn << 5) | rd)
def strb_zero(rt, rn): return enc(0x39000000 | (rn << 5) | rt)      # strb wt, [xn]
def tbz_stub(): return 0

CASES = {
 "movz x0,#0x1234,lsl#32":        (movz(0, 0x1234, 2), "movz x0, #0x1234, lsl #32"),
 "movk x3,#0x5,lsl#16":           (movk(3, 5, 1), "movk x3, #0x5, lsl #16"),
 "mov x1,x2":                     (mov_rr(1, 2), "mov x1, x2"),
 "add x0,x1,x2":                  (add_rrr(0, 1, 2), "add x0, x1, x2"),
 "sub x4,x5,x6":                  (sub_rrr(4, 5, 6), "sub x4, x5, x6"),
 "mul x0,x1,x2":                  (mul_rrr(0, 1, 2), "mul x0, x1, x2"),
 "sdiv x2,x0,x1":                 (sdiv_rrr(2, 0, 1), "sdiv x2, x0, x1"),
 "udiv x2,x0,x1":                 (udiv_rrr(2, 0, 1), "udiv x2, x0, x1"),
 "msub x0,x2,x1,x0":              (msub_rrrr(0, 2, 1, 0), "msub x0, x2, x1, x0"),
 "and x0,x0,x1":                  (and_rrr(0, 0, 1), "and x0, x0, x1"),
 "orr x0,x0,x1":                  (orr_rrr(0, 0, 1), "orr x0, x0, x1"),
 "eor x0,x0,x1":                  (eor_rrr(0, 0, 1), "eor x0, x0, x1"),
 "mvn x0,x0":                     (mvn_rr(0, 0), "mvn x0, x0"),
 "neg x0,x0":                     (neg_rr(0, 0), "neg x0, x0"),
 "lsl x0,x0,x1":                  (lslv(0, 0, 1), "lsl x0, x0, x1"),
 "lsr x0,x0,x1":                  (lsrv(0, 0, 1), "lsr x0, x0, x1"),
 "asr x0,x0,x1":                  (asrv(0, 0, 1), "asr x0, x0, x1"),
 "cmp x0,x1":                     (cmp_rr(0, 1), "cmp x0, x1"),
 "cset x0,lt":                    (cset(0, 11), "cset x0, lt"),
 "cset x0,hs":                    (cset(0, 2), "cset x0, hs"),
 "sub sp,sp,#32":                 (sub_sp(32), "sub sp, sp, #32"),
 "sub x9,x29,#8":                 (sub_ri(9, 29, 8), "sub x9, x29, #8"),
 "add x29,sp,#0":                 (mov_fp_sp(), "add x29, sp, #0"),
 "add sp,x29,#0":                 (mov_sp_fp(), "add sp, x29, #0"),
 "str x0,[x9]":                   (str_zero(0, 9), "str x0, [x9]"),
 "ldr x0,[x9]":                   (ldr_zero(0, 9), "ldr x0, [x9]"),
 "str x0,[sp,#-16]!":             (str_pre16(0), "str x0, [sp, #-16]!"),
 "ldr x0,[sp],#16":               (ldr_post16(0), "ldr x0, [sp], #16"),
 "ldr x3,[sp],#16":               (ldr_post16(3), "ldr x3, [sp], #16"),
 "stp x29,x30,[sp,#-16]!":        (stp_frame(), "stp x29, x30, [sp, #-16]!"),
 "ldp x29,x30,[sp],#16":          (ldp_frame(), "ldp x29, x30, [sp], #16"),
 "b #8":                          (b_(2), "b #8"),
 "bl #-8":                        (bl_(-2), "bl #-8"),
 "b.ge #16":                      (b_cond(10, 4), "b.ge #16"),
 "cbz x0,#12":                    (cbz(0, 3), "cbz x0, #12"),
 "cbnz x1,#-4":                   (cbnz(1, -1), "cbnz x1, #-4"),
 "adr x1,#4096":                  (adr(1, 4096), "adr x1, #4096"),
 "ret":                           (ret(), "ret"),
 "svc #0":                        (svc0(), "svc #0"),
 "sxtb x0,x0":                    (sbfm_sxtb(0, 0), "sxtb x0, x0"),
 "sxth x0,x0":                    (sbfm_sxth(0, 0), "sxth x0, x0"),
 "sxtw x0,x0":                    (sbfm_sxtw(0, 0), "sxtw x0, x0"),
 "ubfm x0,x0,#0,#7":              (ubfm_uxtb(0, 0), "ubfm x0, x0, #0, #7"),
 "ubfm x0,x0,#0,#15":             (ubfm_uxth(0, 0), "ubfm x0, x0, #0, #15"),
 "ubfm x0,x0,#0,#31":             (ubfm_uxtw(0, 0), "ubfm x0, x0, #0, #31"),
 "strb w8,[x4]":                  (strb_zero(8, 4), "strb w8, [x4]"),
}


class Arm64Encodings(unittest.TestCase):
    @unittest.skipUnless(shutil.which("llvm-mc"), "llvm-mc is not installed")
    def test_every_encoding_matches_llvm_mc(self):
        names = list(CASES)
        text = "\n".join(CASES[name][1] for name in names) + "\n"
        done = subprocess.run(["llvm-mc", "-triple=aarch64", "-show-encoding"], input=text, capture_output=True, text=True, check=True)
        got = []
        for line in done.stdout.splitlines():
            if "encoding: [" in line:
                hexes = line.split("encoding: [", 1)[1].split("]", 1)[0]
                got.append(int.from_bytes(bytes(int(h, 16) for h in hexes.split(",")), "little"))
        self.assertEqual(len(got), len(names), "llvm-mc must encode every case")
        for name, word in zip(names, got):
            self.assertEqual(CASES[name][0] & 0xffffffff, word, f"{name}: {CASES[name][1]}")


if __name__ == "__main__":
    unittest.main()
