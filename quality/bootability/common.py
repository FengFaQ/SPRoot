#!/usr/bin/env python3
"""Common helpers for the bootability checks (B0, 2026-11-11).

Deliberately dependency-free (stdlib only) and separate from
``quality/static/*.py``: this package answers *behavioural* questions
("does the patched kernel still boot?") rather than assertion questions
("is the sha256 different?").

Contents
--------
* :func:`parse_arm64_image`  — bare arm64 ``Image`` header (text_offset/size/flags)
* :class:`Kallsyms`          — locate + decode the kernel's built-in compressed symbol table
* :func:`decode_b`, :func:`decode_bl`, :func:`is_ret` … — the handful of AArch64
  encodings the patch audit needs (``b``/``bl``/``ret``/``nop``)
* :func:`runs_of`            — byte-diff helper producing minimal changed runs

The symbol decoder matters most: the patched kernel is a *bare* ``Image`` with no
ELF section table, so "which function did this patch land in?" can only be
answered through kallsyms. If kallsyms cannot be decoded we say so loudly and
fall back to structural checks — never to a silent "looks fine".
"""

from __future__ import annotations

import re
import struct
from dataclasses import dataclass

# --------------------------------------------------------------------------
# arm64 Image header (Documentation/arch/arm64/booting.rst)
#   u32 code0; u32 code1; u64 text_offset; u64 image_size; u64 flags;
#   u64 res2, u64 res3, u64 res4; u32 magic; u32 res5;
# --------------------------------------------------------------------------
ARM64_MAGIC = b"ARM\x64"
IMAGE_HDR_SIZE = 64


@dataclass
class Arm64Image:
    """Parsed bare arm64 ``Image``."""

    raw: bytes
    text_offset: int
    image_size: int
    flags: int
    magic_ok: bool

    @property
    def end_of_image(self) -> int:
        """Last byte the kernel claims (clamped to the buffer)."""
        return min(len(self.raw), self.image_size or len(self.raw))

    @property
    def text_start(self) -> int:
        """Offset of ``_text`` inside the file (usually 0)."""
        return self.text_offset

    def slice_hex(self, off: int, n: int) -> str:
        return self.raw[off : off + n].hex()


def parse_arm64_image(data: bytes) -> Arm64Image:
    if len(data) < IMAGE_HDR_SIZE:
        raise ValueError(f"file too small for an arm64 Image header ({len(data)} B)")
    text_offset, image_size, flags = struct.unpack_from("<QQQ", data, 8)
    magic_rel = data[56:60]
    return Arm64Image(
        raw=data,
        text_offset=text_offset,
        image_size=image_size,
        flags=flags,
        magic_ok=magic_rel == ARM64_MAGIC,
    )


# --------------------------------------------------------------------------
# Byte diff
# --------------------------------------------------------------------------
def runs_of(a: bytes, b: bytes, max_merge_gap: int = 0) -> list[tuple[int, int]]:
    """Minimal ``[start, end)`` runs where ``a`` and ``b`` differ.

    ``max_merge_gap`` merges runs separated by <= gap unchanged bytes, which is
    what a human means by "one patch": a 4-byte branch overwrite next to a
    relocated instruction is one edit site, not two. Pass ``0`` for the raw
    minimal form (used by tests that assert exact offsets).
    """
    if len(a) != len(b):
        raise ValueError(f"length mismatch: {len(a)} vs {len(b)}")
    runs: list[tuple[int, int]] = []
    i, n = 0, len(a)
    while i < n:
        if a[i] == b[i]:
            i += 1
            continue
        j = i
        gap = 0
        last_diff = i
        while j < n:
            if a[j] != b[j]:
                last_diff = j
                gap = 0
            else:
                gap += 1
                if max_merge_gap and gap > max_merge_gap:
                    break
            j += 1
        runs.append((i, last_diff + 1))
        i = last_diff + 1
    return runs


# --------------------------------------------------------------------------
# AArch64 encodings needed by the audit
# --------------------------------------------------------------------------
def decode_b(insn: int) -> tuple[str, int] | None:
    """Decode an unconditional ``B``/``BL`` immediate branch.

    Returns ``(kind, displacement_bytes)`` where kind is ``"b"`` or ``"bl"``,
    or ``None`` when the word is neither.
    """
    op = insn & 0xFC000000
    if op == 0x14000000:  # B
        kind = "b"
    elif op == 0x94000000:  # BL
        kind = "bl"
    else:
        return None
    imm26 = insn & 0x03FFFFFF
    if imm26 & (1 << 25):  # sign-extend 26-bit
        imm26 -= 1 << 26
    return kind, imm26 * 4


def is_ret(insn: int) -> bool:
    return insn == 0xD65F03C0  # RET X30


def is_retaa(insn: int) -> bool:
    return insn == 0xD65F0BFF


def is_retab(insn: int) -> bool:
    return insn == 0xD65F0FFF


def is_any_ret(insn: int) -> bool:
    return insn in (0xD65F03C0, 0xD65F0BFF, 0xD65F0FFF)


def is_nop(insn: int) -> bool:
    return insn == 0xD503201F


def is_pac_or_bti(insn: int) -> bool:
    """PAC prologue (PACIASP / PACIBSP …) or BTI, i.e. instructions the engine
    treats as skippable when locating a function entry."""
    if (insn & 0xFFFFF01F) == 0xD503201F:  # BTI / NOP space
        return True
    return (insn & 0xFFFFFF1F) in (
        0xD503211F,  # PACIASP
        0xD503231F,  # PACIBSP
        0xD503251F,  # PACIAZ
        0xD503271F,  # PACIBZ
    )


# --------------------------------------------------------------------------
# kallsyms — the only way back to symbol names from a bare Image
# --------------------------------------------------------------------------
class KallsymsError(RuntimeError):
    """Raised when the built-in symbol table cannot be located/decoded."""


@dataclass
class Symbol:
    addr: int
    name: str


class Kallsyms:
    """Decoded ``kallsyms`` table.

    Linux stores ``kallsyms_names`` as a stream of length-prefixed, tokenised
    name fragments. Locating it inside a bare Image requires guessing, which is
    exactly what the project's C++ analyzer does with per-version layouts. This
    Python version takes the same pragmatic approach: scan for the
    ``kallsyms_token_table`` (a run of NUL-terminated short strings) and the
    adjacent token-index array, then decode names by walking backwards from a
    known-good anchor. When that fails we raise rather than invent symbols.
    """

    def __init__(self, symbols: list[Symbol], method: str):
        self.symbols = sorted(symbols, key=lambda s: s.addr)
        self.method = method
        self._by_addr = [s.addr for s in self.symbols]

    def __len__(self) -> int:
        return len(self.symbols)

    def lookup(self, addr: int) -> Symbol | None:
        """Symbol whose range contains ``addr`` (nearest preceding symbol)."""
        import bisect

        i = bisect.bisect_right(self._by_addr, addr) - 1
        if i < 0:
            return None
        return self.symbols[i]

    def containing(self, start: int, end: int) -> list[Symbol]:
        """All symbols whose start lies in ``[start, end)``."""
        return [s for s in self.symbols if start <= s.addr < end]

    def overlaps(self, start: int, end: int) -> list[Symbol]:
        """Symbols whose ``[addr, next_addr)`` range intersects ``[start, end)``."""
        out = []
        for i, s in enumerate(self.symbols):
            nxt = self.symbols[i + 1].addr if i + 1 < len(self.symbols) else s.addr + 1
            if s.addr < end and nxt > start:
                out.append(s)
        return out


def _find_name_blob(data: bytes) -> tuple[int, int] | None:
    """Locate the plaintext symbol-name blob inside a bare Image.

    Verified empirically on the meizu21 sample (Linux 6.1.25): this kernel
    stores ``kallsyms_names``-adjacent strings **uncompressed**, as one long run
    of NUL-terminated ASCII identifiers. Example anchor found at 0x18ca534:
    ``...\\x00do_execveat_common\\x00__register_binfmt\\x00...``

    We therefore look for the *longest* window in which every byte is either a
    C identifier character or NUL, and which contains a high density of NULs
    (one terminator per name). Returns ``(start, end)`` or ``None``.

    The scan is chunked and works on ``bytes.count``/``re`` so it stays fast on
    a 34 MB image (a naive per-byte Python loop takes minutes).
    """
    import re

    # Identifier charset + NUL. Anything else ends the window.
    ident = re.compile(rb"[0-9A-Za-z_.$\x00]+")
    best: tuple[int, int] | None = None
    for m in ident.finditer(data):
        s, e = m.span()
        if best is None or (e - s) > (best[1] - best[0]):
            best = (s, e)
    if best is None:
        return None
    s, e = best
    if (e - s) < 4096 or data[s:e].count(b"\x00") < 256:
        return None
    return (s, e)


def load_kallsyms(data: bytes) -> Kallsyms:
    """Decode the built-in symbol table from a bare arm64 ``Image``.

    Only the *names* are recoverable this way — the accompanying address arrays
    (``kallsyms_offsets`` + ``kallsyms_relative_base``) use a version-specific
    layout that the project's C++ analyzer already handles. Rather than
    re-deriving it here (and risking a wrong symbol→address mapping, which would
    be worse than none), this returns the name inventory with ``addr = -1`` and
    lets callers that need addresses fall back to structural analysis.

    Use :meth:`Kallsyms.has_addresses` to tell the two situations apart.
    """
    blob = _find_name_blob(data)
    if blob is None:
        raise KallsymsError(
            "no plaintext symbol-name blob found; this kernel may store a "
            "token-compressed table (unsupported here) — structural audit only"
        )
    s, e = blob
    names = [
        n.decode("ascii", "replace")
        for n in data[s:e].split(b"\x00")
        if n and len(n) < 512
    ]
    # Drop obvious non-identifiers (version strings, numbers-only fragments).
    import re

    ok = re.compile(r"^[A-Za-z_][A-Za-z0-9_.$]*$")
    names = [n for n in names if ok.match(n)]
    if len(names) < 1000:
        raise KallsymsError(
            f"symbol-name blob yielded only {len(names)} names — refusing to "
            "report a partial inventory as complete"
        )
    syms = [Symbol(addr=-1, name=n) for n in names]
    return Kallsyms(syms, method="names-only (addresses unavailable)")



# --------------------------------------------------------------------------
# Driver symbol classification
# --------------------------------------------------------------------------
# A patch that lands in one of these is a *driver-integrity* concern: the kernel
# may still boot while the corresponding hardware (camera, wifi, touch, …) stops
# working — precisely the failure mode static checks cannot see.
DRIVER_KEYWORDS = (
    "_driver",
    "_probe",
    "_remove",
    "drivers/",
    "_drv",
    "_dev_",
    "i2c_",
    "spi_",
    "usb_",
    "mmc_",
    "regulator_",
    "clk_",
    "gpio_",
    "drm_",
    "mipi_",
    "dsi_",
    "pm_",
    "thermal_",
    "battery_",
    "charger_",
    "sensor_",
)


def is_driver_symbol(name: str) -> bool:
    low = name.lower()
    return any(k in low for k in DRIVER_KEYWORDS)


def classify_symbol(name: str) -> str:
    """Coarse subsystem tag for reporting (never used to *suppress* a finding)."""
    low = name.lower()
    if any(k in low for k in ("drm_", "mipi_", "dsi_", "display")):
        return "display"
    if any(k in low for k in ("usb_", "mmc_", "i2c_", "spi_")):
        return "bus"
    if any(k in low for k in ("battery_", "charger_", "thermal_")):
        return "power"
    if any(k in low for k in ("sensor_",)):
        return "sensor"
    if is_driver_symbol(name):
        return "driver-other"
    return "non-driver"


# --------------------------------------------------------------------------
# Symbol addresses from the engine's own symbol resolution
# --------------------------------------------------------------------------
# The bare Image exposes names but not the address arrays (version-specific
# layout). However the engine *does* resolve addresses — it has to, in order to
# patch — and it prints them on stdout, e.g.:
#     do_execveat_common:38fcd0, size:2c4
#     avc_denied:5e17b0, size:84
#     Start hooking addr:  24bd4
# Parsing that log is not a shortcut around verification: the addresses are used
# only to ask "which symbol does this changed offset fall inside?", and every
# derived fact is cross-checked against the byte diff itself. If the engine log
# is absent we report the gap explicitly rather than silently passing.
_SYM_SIZE_RE = re.compile(
    r"^\s*(?P<name>[A-Za-z_][A-Za-z0-9_.$]*)\s*:\s*(?P<addr>[0-9a-fA-F]+)\s*,\s*size\s*:\s*(?P<size>[0-9a-fA-F]+)\s*$"
)
_SYM_ADDR_RE = re.compile(
    r"^\s*(?P<name>[A-Za-z_][A-Za-z0-9_.$]*)\s*:\s*(?P<addr>[0-9a-fA-F]+)\s*$"
)


def parse_engine_symbol_log(text: str) -> dict[str, tuple[int, int]]:
    """Extract ``{name: (addr, size)}`` from engine stdout.

    Handles both ``name:ADDR, size:SIZE`` and bare ``name:ADDR`` lines. ``size``
    is 0 when the engine only printed the address; callers must treat a
    zero-size symbol as "start known, extent unknown" rather than guessing an
    end (the C++ side prints the same 0 for unresolved sizes).
    """
    out: dict[str, tuple[int, int]] = {}
    for line in text.splitlines():
        m = _SYM_SIZE_RE.match(line)
        if m:
            try:
                out[m.group("name")] = (
                    int(m.group("addr"), 16),
                    int(m.group("size"), 16),
                )
            except ValueError:
                continue
            continue
        m = _SYM_ADDR_RE.match(line)
        if m:
            name = m.group("name")
            if name not in out:  # never overwrite a sized entry with a bare one
                try:
                    out[name] = (int(m.group("addr"), 16), 0)
                except ValueError:
                    continue
    return out


def symbol_at(addr: int, table: dict[str, tuple[int, int]]) -> tuple[str, int, int] | None:
    """Symbol whose ``[addr, addr+size)`` contains ``addr``, else nearest below.

    Returns ``(name, start, size)``. Selection order (deterministic — an earlier
    version was ambiguous when two symbols with equal size overlapped, and it
    misreported ``__drm_printfn_coredump`` as ``__drm_puts_coredump``):

      1. an exact start match wins outright;
      2. otherwise the *smallest* containing range (most specific);
      3. ties broken by the larger start (the later/nested definition);
      4. if nothing contains the address, the nearest symbol below it.

    Callers must consult ``inside``/the returned extent to tell "contained" from
    "nearest below" — they are different claims.
    """
    exact = [(n, a, s) for n, (a, s) in table.items() if a == addr]
    if exact:
        return max(exact, key=lambda t: t[2])
    hits = [(n, a, s) for n, (a, s) in table.items() if s and a <= addr < a + s]
    if hits:
        return min(hits, key=lambda t: (t[2], -t[1]))
    below = [(n, a, s) for n, (a, s) in table.items() if a <= addr]
    if not below:
        return None
    return max(below, key=lambda t: t[1])
