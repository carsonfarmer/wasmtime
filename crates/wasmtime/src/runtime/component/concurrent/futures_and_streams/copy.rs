//! Copying stream payloads from one component's linear memory to another's
//! without lifting them into `Val`s.
//!
//! [`lift`] copies the writer's items into a host buffer, followed by the
//! strings and lists they point to, with the pointers rewritten as offsets into
//! the buffer. [`lower`] then writes them to the reader's memory, allocating
//! the strings and lists there. Both follow a [`Plan`] made from the payload's
//! type.
//!
//! Together they behave like `Val::load` followed by `Val::store`: the same
//! checks fail with the same errors, `realloc` is called in the same order with
//! the same arguments and after the same writes, and only values are written,
//! not padding or the unused part of a variant's payload.

use crate::component::func::{LiftContext, LowerContext};
use crate::prelude::*;
use crate::{Result, bail, bail_bug};
use wasmtime_environ::component::{ComponentTypes, InterfaceType, VariantInfo};

/// Same as `func::typed::MAX_STRING_BYTE_LENGTH`.
const MAX_STRING_BYTE_LENGTH: usize = (1 << 31) - 1;

/// Whether payloads of type `ty` can be copied by [`lift`] and [`lower`]:
/// anything without handles, maps or fixed-length lists.
pub(super) fn supported(types: &ComponentTypes, ty: InterfaceType) -> bool {
    let ok = |ty: &InterfaceType| supported(types, *ty);
    match ty {
        InterfaceType::Bool
        | InterfaceType::S8
        | InterfaceType::U8
        | InterfaceType::S16
        | InterfaceType::U16
        | InterfaceType::S32
        | InterfaceType::U32
        | InterfaceType::S64
        | InterfaceType::U64
        | InterfaceType::Float32
        | InterfaceType::Float64
        | InterfaceType::Char
        | InterfaceType::String
        | InterfaceType::Enum(_)
        | InterfaceType::Flags(_) => true,
        InterfaceType::List(i) => ok(&types[i].element),
        InterfaceType::Record(i) => types[i].fields.iter().all(|f| ok(&f.ty)),
        InterfaceType::Tuple(i) => types[i].types.iter().all(ok),
        InterfaceType::Variant(i) => types[i].cases.values().flatten().all(ok),
        InterfaceType::Option(i) => ok(&types[i].ty),
        InterfaceType::Result(i) => [types[i].ok, types[i].err].iter().flatten().all(ok),
        _ => false,
    }
}

/// What to do with each part of a value, at offsets from its start, in order.
#[derive(Default)]
pub(super) struct Plan(Vec<Step>);

enum Step {
    /// Copied as is. Adjacent numbers are merged into one step.
    Copy { at: usize, len: usize },
    /// Normalized to 0 or 1.
    Bool(usize),
    /// Checked to be a `char`.
    Char(usize),
    /// `count` flags in `size` bytes, with the bits past them cleared.
    Flags {
        at: usize,
        size: usize,
        count: usize,
    },
    /// Checked to be UTF-8.
    String(usize),
    /// Checked to be in bounds and aligned, with elements laid out by `elem`.
    List {
        at: usize,
        size: usize,
        align: u32,
        elem: Plan,
    },
    /// A `size`-byte discriminant, which picks the plan for the payload.
    Variant {
        at: usize,
        size: usize,
        payload: usize,
        cases: Vec<Plan>,
    },
}

impl Plan {
    pub(super) fn new(types: &ComponentTypes, ty: InterfaceType) -> Plan {
        let mut plan = Plan::default();
        plan.add(types, ty, 0);
        plan
    }

    fn add(&mut self, types: &ComponentTypes, ty: InterfaceType, at: usize) {
        let size = types.canonical_abi(&ty).size32 as usize;
        let case =
            |ty: Option<InterfaceType>| ty.map_or_else(Plan::default, |t| Plan::new(types, t));
        let step = match ty {
            InterfaceType::Bool => Step::Bool(at),
            InterfaceType::Char => Step::Char(at),
            InterfaceType::String => Step::String(at),
            InterfaceType::List(i) => {
                let elem = types[i].element;
                let abi = types.canonical_abi(&elem);
                Step::List {
                    at,
                    size: abi.size32 as usize,
                    align: abi.align32,
                    elem: Plan::new(types, elem),
                }
            }
            InterfaceType::Record(i) => {
                return self.fields(types, types[i].fields.iter().map(|f| f.ty), at);
            }
            InterfaceType::Tuple(i) => {
                return self.fields(types, types[i].types.iter().copied(), at);
            }
            InterfaceType::Variant(i) => {
                let cases = types[i].cases.values().map(|c| case(*c)).collect();
                variant(&types[i].info, at, cases)
            }
            InterfaceType::Enum(i) => {
                let cases = types[i].names.iter().map(|_| Plan::default()).collect();
                variant(&types[i].info, at, cases)
            }
            InterfaceType::Option(i) => {
                let cases = vec![Plan::default(), case(Some(types[i].ty))];
                variant(&types[i].info, at, cases)
            }
            InterfaceType::Result(i) => {
                let cases = vec![case(types[i].ok), case(types[i].err)];
                variant(&types[i].info, at, cases)
            }
            InterfaceType::Flags(i) => Step::Flags {
                at,
                size,
                count: types[i].names.len(),
            },
            _ => {
                if let Some(Step::Copy { at: prev, len }) = self.0.last_mut()
                    && *prev + *len == at
                {
                    *len += size;
                    return;
                }
                Step::Copy { at, len: size }
            }
        };
        self.0.push(step);
    }

    fn fields(
        &mut self,
        types: &ComponentTypes,
        tys: impl Iterator<Item = InterfaceType>,
        at: usize,
    ) {
        let mut offset = 0;
        for ty in tys {
            let field = types.canonical_abi(&ty).next_field32_size(&mut offset);
            self.add(types, ty, at + field);
        }
    }

    /// Whether this plan only copies bytes.
    fn copies(&self) -> bool {
        self.0.iter().all(|s| matches!(s, Step::Copy { .. }))
    }

    /// Whether lowering may allocate in the reader.
    fn allocates(&self) -> bool {
        self.0.iter().any(|s| match s {
            Step::String(_) | Step::List { .. } => true,
            Step::Variant { cases, .. } => cases.iter().any(Plan::allocates),
            _ => false,
        })
    }
}

fn variant(info: &VariantInfo, at: usize, cases: Vec<Plan>) -> Step {
    Step::Variant {
        at,
        size: usize::from(info.size),
        payload: at + info.payload_offset32 as usize,
        cases,
    }
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(*buf[at..].first_chunk().unwrap())
}

fn ptr_len(buf: &[u8], at: usize) -> (usize, usize) {
    (u32_at(buf, at) as usize, u32_at(buf, at + 4) as usize)
}

fn set_ptr(buf: &mut [u8], at: usize, ptr: usize) -> Result<()> {
    buf[at..][..4].copy_from_slice(&u32::try_from(ptr)?.to_le_bytes());
    Ok(())
}

/// Reads and range-checks the `size`-byte discriminant at `at`.
fn discriminant(buf: &[u8], at: usize, size: usize, cases: usize) -> Result<usize> {
    let mut d = [0; 4];
    d[..size].copy_from_slice(&buf[at..][..size]);
    let d = u32::from_le_bytes(d);
    if d as usize >= cases {
        bail!("discriminant {d} out of range [0..{cases})");
    }
    Ok(d as usize)
}

/// Copies `count` items of `size` bytes, laid out by `plan`, from `address` in
/// the writer's memory, into a host buffer along with what they point to.
pub(super) fn lift(
    cx: &mut LiftContext<'_>,
    plan: &Plan,
    address: usize,
    size: usize,
    count: usize,
) -> Result<Vec<u8>> {
    cx.consume_fuel(size * count)?;
    let mut buf = cx.memory()[address..][..size * count].to_vec();
    if !plan.copies() {
        for i in 0..count {
            lift_value(cx, plan, &mut buf, i * size)?;
        }
    }
    Ok(buf)
}

fn lift_value(cx: &mut LiftContext<'_>, plan: &Plan, buf: &mut Vec<u8>, base: usize) -> Result<()> {
    let mem = cx.memory();
    for step in &plan.0 {
        match *step {
            Step::Copy { .. } => {}
            Step::Bool(at) => buf[base + at] = u8::from(buf[base + at] != 0),
            Step::Char(at) => _ = char::try_from(u32_at(buf, base + at))?,
            Step::Flags { at, size, count } => {
                for (i, byte) in buf[base + at..][..size].iter_mut().enumerate() {
                    *byte &= u8::try_from((1_u16 << count.saturating_sub(8 * i).min(8)) - 1)?;
                }
            }
            Step::String(at) => {
                let (ptr, len) = ptr_len(buf, base + at);
                let bytes = match ptr.checked_add(len) {
                    Some(end) if end <= mem.len() => &mem[ptr..end],
                    _ => bail!("string pointer/length out of bounds of memory"),
                };
                cx.consume_fuel(len)?;
                core::str::from_utf8(bytes)?;
                let start = buf.len();
                set_ptr(buf, base + at, start)?;
                buf.extend_from_slice(bytes);
            }
            Step::List {
                at,
                size,
                align,
                ref elem,
            } => {
                let (ptr, len) = ptr_len(buf, base + at);
                let bytes = match len.checked_mul(size).and_then(|n| ptr.checked_add(n)) {
                    Some(end) if end <= mem.len() => &mem[ptr..end],
                    _ => bail!("list pointer/length out of bounds of memory"),
                };
                cx.consume_fuel(bytes.len())?;
                if ptr % align as usize != 0 {
                    bail!("list pointer is not aligned")
                }
                let start = buf.len();
                set_ptr(buf, base + at, start)?;
                buf.extend_from_slice(bytes);
                if !elem.copies() {
                    for i in 0..len {
                        lift_value(cx, elem, buf, start + i * size)?;
                    }
                }
            }
            Step::Variant {
                at,
                size,
                payload,
                ref cases,
            } => {
                let case = discriminant(buf, base + at, size, cases.len())?;
                lift_value(cx, &cases[case], buf, base + payload)?;
            }
        }
    }
    Ok(())
}

/// Writes `count` items of `size` bytes, laid out by `plan`, from `src` in a
/// buffer made by [`lift`] to `dst` in the reader's memory, allocating what
/// they point to there.
pub(super) fn lower<T>(
    cx: &mut LowerContext<'_, T>,
    plan: &Plan,
    size: usize,
    count: usize,
    buf: &mut [u8],
    src: usize,
    dst: usize,
) -> Result<()> {
    if let [Step::Copy { at: 0, len }] = plan.0[..]
        && len == size
    {
        let len = size * count;
        cx.as_slice_mut()[dst..][..len].copy_from_slice(&buf[src..][..len]);
    } else if !plan.allocates() {
        let mem = cx.as_slice_mut();
        for i in 0..count {
            for step in &plan.0 {
                write(step, buf, mem, src + i * size, dst + i * size)?;
            }
        }
    } else {
        for i in 0..count {
            lower_value(cx, plan, buf, src + i * size, dst + i * size)?;
        }
    }
    Ok(())
}

fn lower_value<T>(
    cx: &mut LowerContext<'_, T>,
    plan: &Plan,
    buf: &mut [u8],
    src: usize,
    dst: usize,
) -> Result<()> {
    // Memory is looked up when it's first needed and after each call into the
    // guest, which can grow it.
    let mut mem: &mut [u8] = &mut [];
    for step in &plan.0 {
        let (at, ptr) = match *step {
            Step::String(at) => {
                let (start, len) = ptr_len(buf, src + at);
                if len > MAX_STRING_BYTE_LENGTH {
                    bail!("string length of {len} too large to copy into wasm");
                }
                let ptr = cx.realloc(0, 0, 1, len)?;
                mem = cx.as_slice_mut();
                mem[ptr..][..len].copy_from_slice(&buf[start..][..len]);
                (at, ptr)
            }
            Step::List {
                at,
                size,
                align,
                ref elem,
            } => {
                let (start, len) = ptr_len(buf, src + at);
                let ptr = cx.realloc(0, 0, align, len * size)?;
                lower(cx, elem, size, len, buf, start, ptr)?;
                mem = cx.as_slice_mut();
                (at, ptr)
            }
            Step::Variant {
                at,
                size,
                payload,
                ref cases,
            } => {
                cx.as_slice_mut()[dst + at..][..size].copy_from_slice(&buf[src + at..][..size]);
                let case = discriminant(buf, src + at, size, cases.len())?;
                lower_value(cx, &cases[case], buf, src + payload, dst + payload)?;
                mem = &mut [];
                continue;
            }
            _ => {
                if mem.is_empty() {
                    mem = cx.as_slice_mut();
                }
                write(step, buf, mem, src, dst)?;
                continue;
            }
        };
        set_ptr(buf, src + at, ptr)?;
        mem[dst + at..][..8].copy_from_slice(&buf[src + at..][..8]);
    }
    Ok(())
}

/// Writes the part of a value at `src` in `buf` that `step` covers, which
/// doesn't point to anything, to `dst` in `mem`.
fn write(step: &Step, buf: &[u8], mem: &mut [u8], src: usize, dst: usize) -> Result<()> {
    let (at, len) = match *step {
        Step::Copy { at, len } => (at, len),
        Step::Bool(at) => (at, 1),
        Step::Char(at) => (at, 4),
        Step::Flags { at, size, .. } => (at, size),
        Step::Variant {
            at,
            size,
            payload,
            ref cases,
        } => {
            let case = &cases[discriminant(buf, src + at, size, cases.len())?];
            for step in &case.0 {
                write(step, buf, mem, src + payload, dst + payload)?;
            }
            (at, size)
        }
        Step::String(_) | Step::List { .. } => bail_bug!("copying a pointer without lowering"),
    };
    mem[dst + at..][..len].copy_from_slice(&buf[src + at..][..len]);
    Ok(())
}
