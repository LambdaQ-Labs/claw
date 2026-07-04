//! Shared value representation wrapping raw bytes + layout.
//!
//! Provides canonical formatting for all Claw value types, usable by the
//! interpreter, dev backend, test helpers, and snapshot tool.

const std = @import("std");
const layout = @import("layout");
const builtins = @import("builtins");
const base = @import("base");

const Layout = layout.Layout;
const Idx = layout.Idx;
const ClawDec = builtins.dec.ClawDec;
const ClawStr = builtins.str.ClawStr;
const ClawList = builtins.list.ClawList;
const i128h = builtins.compiler_rt_128;
const Ident = base.Ident;

const ClawValue = @This();

/// Pointer to raw value bytes (null for zero-sized types).
ptr: ?[*]const u8,
/// Layout describing this value's memory representation.
lay: Layout,
/// Optional layout index for callers that want to preserve the canonical
/// layout handle alongside the materialized Layout value.
layout_idx: ?Idx = null,

/// Wrap an opaque pointer and its layout into a `ClawValue`.
pub fn fromPtr(raw_ptr: *const anyopaque, lay_val: Layout) ClawValue {
    return .{ .ptr = @ptrCast(raw_ptr), .lay = lay_val };
}

/// Wrap an opaque pointer, its layout, and the layout index into a `ClawValue`.
pub fn fromPtrWithIdx(raw_ptr: *const anyopaque, lay_val: Layout, idx: Idx) ClawValue {
    return .{ .ptr = @ptrCast(raw_ptr), .lay = lay_val, .layout_idx = idx };
}

/// Wrap a raw byte pointer and its layout into a `ClawValue`.
pub fn fromRawBytes(raw_ptr: [*]const u8, lay_val: Layout) ClawValue {
    return .{ .ptr = raw_ptr, .lay = lay_val };
}

/// Create a `ClawValue` for a zero-sized type (null pointer).
pub fn zst(lay_val: Layout) ClawValue {
    return .{ .ptr = null, .lay = lay_val };
}

inline fn readAligned(comptime T: type, raw_ptr: [*]const u8) T {
    var result: T = undefined;
    @memcpy(std.mem.asBytes(&result), raw_ptr[0..@sizeOf(T)]);
    return result;
}

/// Read the value as a signed 128-bit integer, widening smaller int types.
pub fn readI128(self: ClawValue) i128 {
    const raw_ptr = self.ptr orelse return 0;
    return switch (self.lay.getScalar().getInt()) {
        .u8 => readAligned(u8, raw_ptr),
        .i8 => readAligned(i8, raw_ptr),
        .u16 => readAligned(u16, raw_ptr),
        .i16 => readAligned(i16, raw_ptr),
        .u32 => readAligned(u32, raw_ptr),
        .i32 => readAligned(i32, raw_ptr),
        .u64 => readAligned(u64, raw_ptr),
        .i64 => readAligned(i64, raw_ptr),
        .i128 => readAligned(i128, raw_ptr),
        .u128 => @bitCast(readAligned(u128, raw_ptr)),
    };
}

/// Read the value as an unsigned 128-bit integer, widening smaller int types.
pub fn readU128(self: ClawValue) u128 {
    const raw_ptr = self.ptr orelse return 0;
    return switch (self.lay.getScalar().getInt()) {
        .u8 => readAligned(u8, raw_ptr),
        .u16 => readAligned(u16, raw_ptr),
        .u32 => readAligned(u32, raw_ptr),
        .u64 => readAligned(u64, raw_ptr),
        .u128 => readAligned(u128, raw_ptr),
        .i8 => @bitCast(@as(i128, readAligned(i8, raw_ptr))),
        .i16 => @bitCast(@as(i128, readAligned(i16, raw_ptr))),
        .i32 => @bitCast(@as(i128, readAligned(i32, raw_ptr))),
        .i64 => @bitCast(@as(i128, readAligned(i64, raw_ptr))),
        .i128 => @bitCast(readAligned(i128, raw_ptr)),
    };
}

/// Read the value as a boolean (any non-zero byte is `true`).
pub fn readBool(self: ClawValue) bool {
    const raw_ptr = self.ptr orelse return false;
    return readAligned(u8, raw_ptr) != 0;
}

/// Read the value as a 32-bit float.
pub fn readF32(self: ClawValue) f32 {
    const raw_ptr = self.ptr orelse return 0;
    return readAligned(f32, raw_ptr);
}

/// Read the value as a 64-bit float.
pub fn readF64(self: ClawValue) f64 {
    const raw_ptr = self.ptr orelse return 0;
    return readAligned(f64, raw_ptr);
}

/// Read the value as a `ClawDec` (i128-backed fixed-point decimal).
pub fn readDec(self: ClawValue) ClawDec {
    const raw_ptr = self.ptr orelse return ClawDec{ .num = 0 };
    return ClawDec{ .num = readAligned(i128, raw_ptr) };
}

/// Reinterpret the value bytes as a `ClawStr`.
pub fn readStr(self: ClawValue) *const ClawStr {
    return @ptrCast(@alignCast(self.ptr.?));
}

/// Reinterpret the value bytes as a `ClawList`.
pub fn readList(self: ClawValue) *const ClawList {
    return @ptrCast(@alignCast(self.ptr.?));
}

/// Read the value as an opaque pointer payload.
pub fn readOpaquePtr(self: ClawValue) usize {
    const raw_ptr = self.ptr orelse return 0;
    return readAligned(usize, raw_ptr);
}

/// Lightweight context for formatting values — carries only layout metadata.
pub const FormatContext = struct {
    layout_store: *const layout.Store,
    ident_store: ?*const Ident.Store = null,
};

/// Errors that can occur during value formatting.
pub const FormatError = error{OutOfMemory};

/// Format this value into a newly-allocated string using canonical Claw syntax.
pub fn format(self: ClawValue, allocator: std.mem.Allocator, ctx: FormatContext) FormatError![]u8 {
    // --- Scalars ---
    if (self.lay.tag == .scalar) {
        const scalar = self.lay.getScalar();
        switch (scalar.tag) {
            .str => {
                const rs = self.readStr();
                const s = rs.asSlice();
                var buf = std.array_list.AlignedManaged(u8, null).init(allocator);
                errdefer buf.deinit();
                try buf.append('"');
                for (s) |ch| {
                    switch (ch) {
                        '\\' => try buf.appendSlice("\\\\"),
                        '"' => try buf.appendSlice("\\\""),
                        else => try buf.append(ch),
                    }
                }
                try buf.append('"');
                return buf.toOwnedSlice();
            },
            .int => {
                const precision = scalar.getInt();
                return switch (precision) {
                    .u64, .u128 => try std.fmt.allocPrint(allocator, "{d}", .{self.readU128()}),
                    else => try std.fmt.allocPrint(allocator, "{d}", .{self.readI128()}),
                };
            },
            .frac => {
                return switch (scalar.getFrac()) {
                    .f32 => blk: {
                        var buf: [400]u8 = undefined;
                        const slice = i128h.f32_to_str(&buf, self.readF32());
                        break :blk try allocator.dupe(u8, slice);
                    },
                    .f64 => blk: {
                        var buf: [400]u8 = undefined;
                        const slice = i128h.f64_to_str(&buf, self.readF64());
                        break :blk try allocator.dupe(u8, slice);
                    },
                    .dec => {
                        const dec = self.readDec();
                        var buf: [ClawDec.max_str_length]u8 = undefined;
                        const slice = dec.format_to_buf(&buf);
                        return try allocator.dupe(u8, slice);
                    },
                };
            },
            .opaque_ptr => return try allocator.dupe(u8, "<opaque>"),
        }
    }

    // --- Structs (unified records and tuples) ---
    if (self.lay.tag == .struct_) {
        const struct_data = ctx.layout_store.getStructData(self.lay.getStruct().idx);
        const fields = ctx.layout_store.struct_fields.sliceRange(struct_data.getFields());
        if (struct_data.fields.count == 0) {
            return try allocator.dupe(u8, "{}");
        }

        var out = std.array_list.AlignedManaged(u8, null).init(allocator);
        errdefer out.deinit();
        try out.append('(');
        const count = fields.len;
        // Iterate by original semantic index rather than sorted layout order.
        var original_idx: usize = 0;
        while (original_idx < count) : (original_idx += 1) {
            const sorted_idx = blk: {
                for (0..count) |si| {
                    if (fields.get(si).index == original_idx) break :blk si;
                }
                unreachable;
            };
            const fld = fields.get(sorted_idx);
            const elem_layout = ctx.layout_store.getLayout(fld.layout);
            const elem_offset = ctx.layout_store.getStructFieldOffset(self.lay.getStruct().idx, @intCast(sorted_idx));
            const base_ptr = self.ptr.?;
            const elem_ptr = base_ptr + elem_offset;
            const elem_val = ClawValue{ .ptr = elem_ptr, .lay = elem_layout };
            const rendered = try elem_val.format(allocator, ctx);
            defer allocator.free(rendered);
            try out.appendSlice(rendered);
            if (original_idx + 1 < count) try out.appendSlice(", ");
        }
        try out.append(')');
        return out.toOwnedSlice();
    }

    // --- Lists ---
    if (self.lay.tag == .list) {
        var out = std.array_list.AlignedManaged(u8, null).init(allocator);
        errdefer out.deinit();
        const roc_list = self.readList();
        const len = roc_list.len();
        try out.append('[');
        if (len > 0) {
            const elem_layout_idx = self.lay.getIdx();
            const elem_layout = ctx.layout_store.getLayout(elem_layout_idx);
            const elem_size = ctx.layout_store.layoutSize(elem_layout);
            var i: usize = 0;
            while (i < len) : (i += 1) {
                if (roc_list.bytes) |bytes| {
                    const elem_ptr: [*]const u8 = bytes + i * elem_size;
                    const elem_val = ClawValue{ .ptr = elem_ptr, .lay = elem_layout };
                    const rendered = try elem_val.format(allocator, ctx);
                    defer allocator.free(rendered);
                    try out.appendSlice(rendered);
                    if (i + 1 < len) try out.appendSlice(", ");
                }
            }
        }
        try out.append(']');
        return out.toOwnedSlice();
    }

    // --- List of ZST ---
    if (self.lay.tag == .list_of_zst) {
        const roc_list = self.readList();
        const len = roc_list.len();
        var out = std.array_list.AlignedManaged(u8, null).init(allocator);
        errdefer out.deinit();
        try out.append('[');
        if (len > 0) {
            // list_of_zst does not carry concrete element data; render canonical ZST
            // placeholders so interpreter/dev/wasm textual comparisons stay aligned.
            var i: usize = 0;
            while (i < len) : (i += 1) {
                try out.appendSlice("{}");
                if (i + 1 < len) try out.appendSlice(", ");
            }
        }
        try out.append(']');
        return out.toOwnedSlice();
    }

    // Records are now handled in the struct_ block above

    // --- Box ---
    if (self.lay.tag == .box) {
        var out = std.array_list.AlignedManaged(u8, null).init(allocator);
        errdefer out.deinit();
        try out.appendSlice("Box(");
        const elem_layout_idx = self.lay.getIdx();
        const elem_layout = ctx.layout_store.getLayout(elem_layout_idx);
        const elem_size = ctx.layout_store.layoutSize(elem_layout);
        if (elem_size > 0) {
            if (self.getBoxedData()) |data_ptr| {
                const elem_val = ClawValue{ .ptr = data_ptr, .lay = elem_layout };
                const rendered = try elem_val.format(allocator, ctx);
                defer allocator.free(rendered);
                try out.appendSlice(rendered);
            } else {
                unreachable;
            }
        } else {
            const elem_val = ClawValue.zst(elem_layout);
            const rendered = try elem_val.format(allocator, ctx);
            defer allocator.free(rendered);
            try out.appendSlice(rendered);
        }
        try out.append(')');
        return out.toOwnedSlice();
    }

    // --- Box of ZST ---
    if (self.lay.tag == .box_of_zst) {
        return try allocator.dupe(u8, "Box({})");
    }

    // --- Tag union ---
    if (self.lay.tag == .tag_union) {
        unreachable; // tag unions must be formatted via formatTagUnion with type info
    }

    // --- ZST ---
    if (self.lay.tag == .zst) {
        return try allocator.dupe(u8, "{}");
    }

    unreachable; // all layout types must be handled
}

/// Compare two ClawValues for structural equality.
/// The `FormatContext` is needed because composite types require the
/// `layout_store` to determine field offsets and element sizes.
pub fn equals(self: ClawValue, other: ClawValue, ctx: FormatContext) bool {
    // Tags must match
    if (self.lay.tag != other.lay.tag) return false;

    switch (self.lay.tag) {
        .scalar => {
            const s_scalar = self.lay.getScalar();
            const o_scalar = other.lay.getScalar();
            if (s_scalar.tag != o_scalar.tag) return false;
            return switch (s_scalar.tag) {
                .str => self.readStr().eql(other.readStr().*),
                .int => {
                    // Compare as i128 (widened)
                    return self.readI128() == other.readI128();
                },
                .frac => {
                    if (s_scalar.getFrac() != o_scalar.getFrac()) return false;
                    return switch (s_scalar.getFrac()) {
                        .f32 => @as(u32, @bitCast(self.readF32())) == @as(u32, @bitCast(other.readF32())),
                        .f64 => @as(u64, @bitCast(self.readF64())) == @as(u64, @bitCast(other.readF64())),
                        .dec => self.readDec().num == other.readDec().num,
                    };
                },
                .opaque_ptr => return self.readOpaquePtr() == other.readOpaquePtr(),
            };
        },
        .erased_callable => unreachable, // Function values are not equality-comparable Claw values.
        .zst => return true,
        .struct_ => {
            const s_fields = ctx.layout_store.struct_fields.sliceRange(
                ctx.layout_store.getStructData(self.lay.getStruct().idx).getFields(),
            );
            const o_fields = ctx.layout_store.struct_fields.sliceRange(
                ctx.layout_store.getStructData(other.lay.getStruct().idx).getFields(),
            );
            if (s_fields.len != o_fields.len) return false;
            for (0..s_fields.len) |i| {
                const s_fld = s_fields.get(i);
                const o_fld = o_fields.get(i);
                const s_field_layout = ctx.layout_store.getLayout(s_fld.layout);
                const o_field_layout = ctx.layout_store.getLayout(o_fld.layout);
                const s_offset = ctx.layout_store.getStructFieldOffset(self.lay.getStruct().idx, @intCast(i));
                const o_offset = ctx.layout_store.getStructFieldOffset(other.lay.getStruct().idx, @intCast(i));
                const s_field = ClawValue{ .ptr = self.ptr.? + s_offset, .lay = s_field_layout };
                const o_field = ClawValue{ .ptr = other.ptr.? + o_offset, .lay = o_field_layout };
                if (!s_field.equals(o_field, ctx)) return false;
            }
            return true;
        },
        .list => {
            const s_list = self.readList();
            const o_list = other.readList();
            if (s_list.len() != o_list.len()) return false;
            const len = s_list.len();
            if (len == 0) return true;
            const s_elem_layout = ctx.layout_store.getLayout(self.lay.getIdx());
            const o_elem_layout = ctx.layout_store.getLayout(other.lay.getIdx());
            const s_elem_size = ctx.layout_store.layoutSize(s_elem_layout);
            const o_elem_size = ctx.layout_store.layoutSize(o_elem_layout);
            const s_bytes = s_list.bytes orelse return false;
            const o_bytes = o_list.bytes orelse return false;
            for (0..len) |i| {
                const s_elem = ClawValue{ .ptr = s_bytes + i * s_elem_size, .lay = s_elem_layout };
                const o_elem = ClawValue{ .ptr = o_bytes + i * o_elem_size, .lay = o_elem_layout };
                if (!s_elem.equals(o_elem, ctx)) return false;
            }
            return true;
        },
        .list_of_zst => {
            return self.readList().len() == other.readList().len();
        },
        // .record is now handled by .struct_ above
        .box => {
            const s_inner_layout = ctx.layout_store.getLayout(self.lay.getIdx());
            const o_inner_layout = ctx.layout_store.getLayout(other.lay.getIdx());
            const s_inner_size = ctx.layout_store.layoutSize(s_inner_layout);
            if (s_inner_size == 0) return true; // Both are boxes of ZST
            const s_data = self.getBoxedData() orelse return other.getBoxedData() == null;
            const o_data = other.getBoxedData() orelse return false;
            const s_inner = ClawValue{ .ptr = s_data, .lay = s_inner_layout };
            const o_inner = ClawValue{ .ptr = o_data, .lay = o_inner_layout };
            return s_inner.equals(o_inner, ctx);
        },
        .box_of_zst => return true,
        .tag_union => {
            const s_tu_idx = self.lay.getTagUnion().idx;
            const o_tu_idx = other.lay.getTagUnion().idx;
            const s_tu_data = ctx.layout_store.getTagUnionData(s_tu_idx);
            const o_tu_data = ctx.layout_store.getTagUnionData(o_tu_idx);
            const s_disc_offset = ctx.layout_store.getTagUnionDiscriminantOffset(s_tu_idx);
            const o_disc_offset = ctx.layout_store.getTagUnionDiscriminantOffset(o_tu_idx);
            const s_ptr = self.ptr orelse return other.ptr == null;
            const o_ptr = other.ptr orelse return false;
            const s_disc = s_tu_data.readDiscriminantFromPtr(s_ptr + s_disc_offset);
            const o_disc = o_tu_data.readDiscriminantFromPtr(o_ptr + o_disc_offset);
            if (s_disc != o_disc) return false;
            // Compare payload for the active variant
            const s_variants = ctx.layout_store.getTagUnionVariants(s_tu_data);
            const o_variants = ctx.layout_store.getTagUnionVariants(o_tu_data);
            const s_payload_layout = ctx.layout_store.getLayout(s_variants.get(s_disc).payload_layout);
            const o_payload_layout = ctx.layout_store.getLayout(o_variants.get(o_disc).payload_layout);
            const s_payload = ClawValue{ .ptr = s_ptr, .lay = s_payload_layout };
            const o_payload = ClawValue{ .ptr = o_ptr, .lay = o_payload_layout };
            return s_payload.equals(o_payload, ctx);
        },
        .closure => return false, // Closures are not compared structurally
        .ptr => unreachable, // Compiler-internal TRMC pointer; never a comparable runtime value.
    }
}

/// Dereference the box pointer. Returns the inner data pointer or null.
fn getBoxedData(self: ClawValue) ?[*]const u8 {
    if (self.ptr) |ptr| {
        const slot: *const usize = @ptrCast(@alignCast(ptr));
        if (slot.* == 0) return null;
        return @ptrFromInt(slot.*);
    }
    return null;
}

test "readBool reads discriminant byte" {
    var true_byte: [1]u8 = .{1};
    var false_byte: [1]u8 = .{0};
    const bool_layout = Layout.boolType();
    try std.testing.expect((ClawValue{ .ptr = &true_byte, .lay = bool_layout }).readBool());
    try std.testing.expect(!(ClawValue{ .ptr = &false_byte, .lay = bool_layout }).readBool());
}

test "format i64" {
    const allocator = std.testing.allocator;
    const i64_layout = Layout.int(.i64);
    var bytes: [@sizeOf(i64)]u8 = undefined;
    @memcpy(&bytes, std.mem.asBytes(&@as(i64, -42)));
    const val = ClawValue{ .ptr = &bytes, .lay = i64_layout };
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    const result = try val.format(allocator, ctx);
    defer allocator.free(result);
    try std.testing.expectEqualStrings("-42", result);
}

test "format u64" {
    const allocator = std.testing.allocator;
    const u64_layout = Layout.int(.u64);
    var bytes: [@sizeOf(u64)]u8 = undefined;
    @memcpy(&bytes, std.mem.asBytes(&@as(u64, 42)));
    const val = ClawValue{ .ptr = &bytes, .lay = u64_layout };
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    const result = try val.format(allocator, ctx);
    defer allocator.free(result);
    try std.testing.expectEqualStrings("42", result);
}

test "format dec with strip" {
    const allocator = std.testing.allocator;
    const dec_layout = Layout.frac(.dec);
    // 3 as Dec = 3 * 10^18
    const dec_val: i128 = 3 * ClawDec.one_point_zero_i128;
    var bytes: [@sizeOf(i128)]u8 = undefined;
    @memcpy(&bytes, std.mem.asBytes(&dec_val));
    const val = ClawValue{ .ptr = &bytes, .lay = dec_layout };
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    const result = try val.format(allocator, ctx);
    defer allocator.free(result);
    try std.testing.expectEqualStrings("3.0", result);
}

test "format dec fractional" {
    const allocator = std.testing.allocator;
    const dec_layout = Layout.frac(.dec);
    // 3.14 as Dec
    const dec_val: i128 = 3_140_000_000_000_000_000;
    var bytes: [@sizeOf(i128)]u8 = undefined;
    @memcpy(&bytes, std.mem.asBytes(&dec_val));
    const val = ClawValue{ .ptr = &bytes, .lay = dec_layout };
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    const result = try val.format(allocator, ctx);
    defer allocator.free(result);
    try std.testing.expectEqualStrings("3.14", result);
}

test "format zst" {
    const allocator = std.testing.allocator;
    const zst_layout = Layout.zst();
    const val = ClawValue.zst(zst_layout);
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    const result = try val.format(allocator, ctx);
    defer allocator.free(result);
    try std.testing.expectEqualStrings("{}", result);
}

test "format box_of_zst" {
    const allocator = std.testing.allocator;
    const box_zst_layout = Layout.boxOfZst();
    const val = ClawValue.zst(box_zst_layout);
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    const result = try val.format(allocator, ctx);
    defer allocator.free(result);
    try std.testing.expectEqualStrings("Box({})", result);
}

test "equals i64" {
    const i64_layout = Layout.int(.i64);
    var a: [@sizeOf(i64)]u8 = undefined;
    var b: [@sizeOf(i64)]u8 = undefined;
    var c: [@sizeOf(i64)]u8 = undefined;
    @memcpy(&a, std.mem.asBytes(&@as(i64, 42)));
    @memcpy(&b, std.mem.asBytes(&@as(i64, 42)));
    @memcpy(&c, std.mem.asBytes(&@as(i64, -1)));
    const va = ClawValue{ .ptr = &a, .lay = i64_layout };
    const vb = ClawValue{ .ptr = &b, .lay = i64_layout };
    const vc = ClawValue{ .ptr = &c, .lay = i64_layout };
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    try std.testing.expect(va.equals(vb, ctx));
    try std.testing.expect(!va.equals(vc, ctx));
}

test "equals f64" {
    const f64_layout = Layout.frac(.f64);
    var a: [@sizeOf(f64)]u8 = undefined;
    var b: [@sizeOf(f64)]u8 = undefined;
    var c: [@sizeOf(f64)]u8 = undefined;
    @memcpy(&a, std.mem.asBytes(&@as(f64, 3.14)));
    @memcpy(&b, std.mem.asBytes(&@as(f64, 3.14)));
    @memcpy(&c, std.mem.asBytes(&@as(f64, 2.71)));
    const va = ClawValue{ .ptr = &a, .lay = f64_layout };
    const vb = ClawValue{ .ptr = &b, .lay = f64_layout };
    const vc = ClawValue{ .ptr = &c, .lay = f64_layout };
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    try std.testing.expect(va.equals(vb, ctx));
    try std.testing.expect(!va.equals(vc, ctx));
}

test "equals dec" {
    const dec_layout = Layout.frac(.dec);
    const dec_a: i128 = 3 * ClawDec.one_point_zero_i128;
    const dec_b: i128 = 3 * ClawDec.one_point_zero_i128;
    const dec_c: i128 = 5 * ClawDec.one_point_zero_i128;
    var a: [@sizeOf(i128)]u8 = undefined;
    var b: [@sizeOf(i128)]u8 = undefined;
    var c: [@sizeOf(i128)]u8 = undefined;
    @memcpy(&a, std.mem.asBytes(&dec_a));
    @memcpy(&b, std.mem.asBytes(&dec_b));
    @memcpy(&c, std.mem.asBytes(&dec_c));
    const va = ClawValue{ .ptr = &a, .lay = dec_layout };
    const vb = ClawValue{ .ptr = &b, .lay = dec_layout };
    const vc = ClawValue{ .ptr = &c, .lay = dec_layout };
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    try std.testing.expect(va.equals(vb, ctx));
    try std.testing.expect(!va.equals(vc, ctx));
}

test "equals zst" {
    const zst_layout = Layout.zst();
    const va = ClawValue.zst(zst_layout);
    const vb = ClawValue.zst(zst_layout);
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    try std.testing.expect(va.equals(vb, ctx));
}

test "equals mismatched tags" {
    const zst_layout = Layout.zst();
    const box_zst_layout = Layout.boxOfZst();
    const va = ClawValue.zst(zst_layout);
    const vb = ClawValue.zst(box_zst_layout);
    const ctx = FormatContext{ .layout_store = undefined, .ident_store = null };
    try std.testing.expect(!va.equals(vb, ctx));
}
