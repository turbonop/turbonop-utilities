//! StackMapTable frames (JVMS 4.7.4).

use turbonop_core::{ErrorKind, Result};

use crate::reader::Cursor;

/// A verification type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VerificationType {
    /// `top` (0).
    Top,
    /// `int` (1).
    Integer,
    /// `float` (2).
    Float,
    /// `double` (3).
    Double,
    /// `long` (4).
    Long,
    /// `null` (5).
    Null,
    /// `uninitializedThis` (6).
    UninitializedThis,
    /// A class (7): its Class constant.
    Object(u16),
    /// An object a `new` at this offset made, not yet initialized (8).
    Uninitialized(u16),
}

impl VerificationType {
    fn parse(c: &mut Cursor<'_>) -> Result<Self> {
        Ok(match c.u8("verification type")? {
            0 => VerificationType::Top,
            1 => VerificationType::Integer,
            2 => VerificationType::Float,
            3 => VerificationType::Double,
            4 => VerificationType::Long,
            5 => VerificationType::Null,
            6 => VerificationType::UninitializedThis,
            7 => VerificationType::Object(c.u16("verification type")?),
            8 => VerificationType::Uninitialized(c.u16("verification type")?),
            _ => return Err(c.error(ErrorKind::Malformed, "verification type")),
        })
    }
}

/// A frame's kind and what it holds beyond its offset delta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// `same_frame` (0-63) and `same_frame_extended` (251).
    Same,
    /// `same_locals_1_stack_item_frame` (64-127, and 247 extended).
    SameLocals1StackItem(VerificationType),
    /// `chop_frame` (248-250): this many locals gone.
    Chop(u8),
    /// `append_frame` (252-254): these locals added.
    Append(Vec<VerificationType>),
    /// `full_frame` (255).
    Full {
        /// The locals.
        locals: Vec<VerificationType>,
        /// The stack.
        stack: Vec<VerificationType>,
    },
}

/// A stack map frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackMapFrame {
    /// Its `frame_type` byte.
    pub frame_type: u8,
    /// The offset delta (implicit in the type for 0-127).
    pub offset_delta: u16,
    /// What it holds.
    pub kind: FrameKind,
}

impl StackMapFrame {
    /// The frame type's name as javap writes it (`same_locals_1_stack_item`).
    pub fn type_name(&self) -> &'static str {
        match self.frame_type {
            0..=63 => "same",
            64..=127 => "same_locals_1_stack_item",
            247 => "same_locals_1_stack_item_frame_extended",
            248..=250 => "chop",
            251 => "same_frame_extended",
            252..=254 => "append",
            255 => "full_frame",
            _ => "reserved",
        }
    }
}

fn types(c: &mut Cursor<'_>, n: usize) -> Result<Vec<VerificationType>> {
    c.check_count(n, 1, "verification types")?;
    (0..n).map(|_| VerificationType::parse(c)).collect()
}

/// The frames of a StackMapTable attribute's data.
pub(crate) fn parse_stack_map_table(c: &mut Cursor<'_>) -> Result<Vec<StackMapFrame>> {
    let n = c.count16(1, "stack map frames")?;
    let mut frames = Vec::with_capacity(n);
    for _ in 0..n {
        let t = c.u8("frame type")?;
        let (offset_delta, kind) = match t {
            0..=63 => (u16::from(t), FrameKind::Same),
            64..=127 => (
                u16::from(t - 64),
                FrameKind::SameLocals1StackItem(VerificationType::parse(c)?),
            ),
            128..=246 => return Err(c.error(ErrorKind::Malformed, "reserved frame type")),
            247 => {
                let d = c.u16("offset delta")?;
                (
                    d,
                    FrameKind::SameLocals1StackItem(VerificationType::parse(c)?),
                )
            }
            248..=250 => (c.u16("offset delta")?, FrameKind::Chop(251 - t)),
            251 => (c.u16("offset delta")?, FrameKind::Same),
            252..=254 => {
                let d = c.u16("offset delta")?;
                (d, FrameKind::Append(types(c, usize::from(t - 251))?))
            }
            255 => {
                let d = c.u16("offset delta")?;
                let nl = c.count16(1, "full frame locals")?;
                let locals = types(c, nl)?;
                let ns = c.count16(1, "full frame stack")?;
                let stack = types(c, ns)?;
                (d, FrameKind::Full { locals, stack })
            }
        };
        frames.push(StackMapFrame {
            frame_type: t,
            offset_delta,
            kind,
        });
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_type() {
        let bytes = [
            0, 7, // 7 frames
            5, // same, delta 5
            70, 1, // same_locals_1_stack_item, delta 6, int
            247, 1, 0, 7, 0, 9, // extended, delta 256, Object #9
            249, 0, 3, // chop 2
            251, 2, 0, // same_frame_extended, delta 512
            253, 0, 1, 4, 8, 0, 12, // append long, Uninitialized(12)
            255, 0, 2, 0, 2, 6, 3, 0, 1, 5, // full: [this, double], [null]
        ];
        let mut c = Cursor::new(&bytes);
        let f = parse_stack_map_table(&mut c).unwrap();
        c.finish("x").unwrap();
        let summary: Vec<(&str, u16)> = f.iter().map(|f| (f.type_name(), f.offset_delta)).collect();
        assert_eq!(
            summary,
            [
                ("same", 5),
                ("same_locals_1_stack_item", 6),
                ("same_locals_1_stack_item_frame_extended", 256),
                ("chop", 3),
                ("same_frame_extended", 512),
                ("append", 1),
                ("full_frame", 2),
            ]
        );
        assert_eq!(f[3].kind, FrameKind::Chop(2));
        assert_eq!(
            f[5].kind,
            FrameKind::Append(vec![
                VerificationType::Long,
                VerificationType::Uninitialized(12)
            ])
        );
        assert_eq!(
            f[6].kind,
            FrameKind::Full {
                locals: vec![
                    VerificationType::UninitializedThis,
                    VerificationType::Double
                ],
                stack: vec![VerificationType::Null]
            }
        );
        assert!(parse_stack_map_table(&mut Cursor::new(&[0, 1, 200])).is_err());
        assert!(parse_stack_map_table(&mut Cursor::new(&[0, 1, 64, 9])).is_err());
        assert!(parse_stack_map_table(&mut Cursor::new(&[0, 1, 255, 0, 0, 0xFF, 0xFF])).is_err());
    }
}
