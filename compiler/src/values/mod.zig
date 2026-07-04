//! Shared value formatting module for Roc runtime values.
//!
//! Provides a common `ClawValue` type that wraps raw bytes + layout and a
//! canonical `format()` function used by the interpreter, dev backend, test
//! helpers, and the snapshot tool.

const std = @import("std");

pub const ClawValue = @import("ClawValue.zig");

test "values tests" {
    std.testing.refAllDecls(@This());
    std.testing.refAllDecls(@import("ClawValue.zig"));
}
