//! The universal TypeScript value representation.
//!
//! TypeScript values are NaN-boxed 64-bit words (similar to JavaScriptCore /
//! V8 Smi encoding).  This module defines the tag bits and boxing/unboxing
//! helpers that the compiler will emit calls to.
//!
//! Layout (NaN-boxing):
//!   - If the top 13 bits are all 1 (quiet NaN range) and bit 50 is 1 →
//!     tagged pointer or special value.
//!   - Otherwise → IEEE-754 double (JS `number`).
//!
//! Tags (bits 49..48 of the quiet-NaN word):
//!   00 → undefined / null (bit 47: 0=undefined, 1=null)
//!   01 → boolean         (bit 0: value)
//!   10 → pointer to heap object
//!   11 → small integer (int32 in lower 32 bits)

/// Opaque 64-bit value type.  The compiler treats every TS value as a `TsVal`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TsVal(pub u64);

// Quiet NaN mask: bits 63..51 all set, plus the quiet bit (bit 50).
const NAN_MASK:       u64 = 0x7FF8_0000_0000_0000;
const TAG_MASK:       u64 = 0x0006_0000_0000_0000;
const TAG_UNDEFINED:  u64 = 0x0000_0000_0000_0000;
const TAG_NULL:       u64 = 0x0001_0000_0000_0000;
const TAG_BOOL:       u64 = 0x0002_0000_0000_0000;
const TAG_PTR:        u64 = 0x0004_0000_0000_0000;
const TAG_INT:        u64 = 0x0006_0000_0000_0000;

pub const UNDEFINED: TsVal = TsVal(NAN_MASK | TAG_UNDEFINED);
pub const NULL:      TsVal = TsVal(NAN_MASK | TAG_NULL);
pub const TRUE:      TsVal = TsVal(NAN_MASK | TAG_BOOL | 1);
pub const FALSE:     TsVal = TsVal(NAN_MASK | TAG_BOOL | 0);

impl TsVal {
    #[inline]
    pub fn from_f64(n: f64) -> Self {
        Self(n.to_bits())
    }

    #[inline]
    pub fn from_i32(n: i32) -> Self {
        Self(NAN_MASK | TAG_INT | (n as u32 as u64))
    }

    #[inline]
    pub fn from_bool(b: bool) -> Self {
        if b { TRUE } else { FALSE }
    }

    #[inline]
    pub fn from_ptr(p: *mut u8) -> Self {
        Self(NAN_MASK | TAG_PTR | (p as u64 & 0x0000_FFFF_FFFF_FFFF))
    }

    #[inline]
    fn is_nan_boxed(self) -> bool {
        (self.0 & NAN_MASK) == NAN_MASK
    }

    #[inline]
    pub fn is_number(self) -> bool {
        !self.is_nan_boxed()
    }

    #[inline]
    pub fn is_undefined(self) -> bool {
        self.is_nan_boxed() && (self.0 & (TAG_MASK | 1)) == TAG_UNDEFINED
    }

    #[inline]
    pub fn is_null(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_NULL
    }

    #[inline]
    pub fn is_bool(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_BOOL
    }

    #[inline]
    pub fn is_ptr(self) -> bool {
        self.is_nan_boxed() && (self.0 & TAG_MASK) == TAG_PTR
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        f64::from_bits(self.0)
    }

    #[inline]
    pub fn as_bool(self) -> bool {
        (self.0 & 1) != 0
    }

    #[inline]
    pub fn as_ptr(self) -> *mut u8 {
        (self.0 & 0x0000_FFFF_FFFF_FFFF) as *mut u8
    }
}
