//! `.fox2` DataSet files: lists of entities with typed properties.
//!
//! Layout (little-endian):
//! * header (32 bytes): magic `0x786f62f2`, `0x35`, entity count, string table
//!   offset, entity data offset;
//! * entities: 64-byte header (class `StrCode64`, address, version, static and
//!   dynamic property counts, total size), then properties;
//! * property: 32-byte header (name `StrCode64`, data type, container type,
//!   value count, total size), then values, padded to 16 bytes;
//! * string table: `(StrCode64, length, bytes)` records ending with hash 0.
//!   String-like values store only the hash; the literal lives here.

use std::collections::HashMap;
use std::fmt::Write;

use crate::NameTable;

#[derive(Debug, thiserror::Error)]
pub enum Fox2Error {
    #[error("not a fox2 file")]
    BadMagic,
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("unknown data type {0} at {1:#x}")]
    UnknownType(u8, usize),
}

type Result<T> = std::result::Result<T, Fox2Error>;

const MAGIC: u32 = 0x786f_62f2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    StaticArray,
    DynamicArray,
    StringMap,
    List,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Bool(bool),
    /// `StrCode64` of the string; see [`DataSet::string`].
    String(u64),
    Path(u64),
    EntityPtr(u64),
    Vector3([f32; 3]),
    Vector4([f32; 4]),
    Quat([f32; 4]),
    Matrix3([f32; 9]),
    Matrix4([f32; 16]),
    Color([f32; 4]),
    FilePtr(u64),
    EntityHandle(u64),
    EntityLink {
        package: u64,
        archive: u64,
        name: u64,
        handle: u64,
    },
    WideVector3 {
        xyz: [f32; 3],
        a: u16,
        b: u16,
    },
}

#[derive(Clone, Debug)]
pub struct Property {
    pub name: u64,
    pub container: ContainerKind,
    /// StringMap entries carry a key hash; other containers do not.
    pub values: Vec<(Option<u64>, Value)>,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub class: u64,
    pub address: u32,
    pub version: i16,
    pub static_properties: Vec<Property>,
    pub dynamic_properties: Vec<Property>,
}

impl Entity {
    pub fn property(&self, name: &str) -> Option<&Property> {
        let h = fox_hash::str_code64(name.as_bytes());
        self.static_properties
            .iter()
            .chain(&self.dynamic_properties)
            .find(|p| p.name == h)
    }
}

#[derive(Clone, Debug, Default)]
pub struct DataSet {
    pub entities: Vec<Entity>,
    /// The file's string table: literals of String/Path/FilePtr values.
    pub strings: HashMap<u64, String>,
}

struct Reader<'a> {
    b: &'a [u8],
}

impl Reader<'_> {
    fn bytes<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        self.b
            .get(at..at + N)
            .map(|s| s.try_into().unwrap())
            .ok_or(Fox2Error::Truncated(at))
    }
    fn u8(&self, at: usize) -> Result<u8> {
        Ok(self.bytes::<1>(at)?[0])
    }
    fn u16(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(at)?))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(at)?))
    }
    fn u64(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(at)?))
    }
    fn f32(&self, at: usize) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(at)?))
    }
    fn floats<const N: usize>(&self, at: usize) -> Result<[f32; N]> {
        let mut out = [0f32; N];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.f32(at + 4 * i)?;
        }
        Ok(out)
    }

    /// Read one value of `ty` at `at`; returns the value and its size.
    fn value(&self, ty: u8, at: usize) -> Result<(Value, usize)> {
        Ok(match ty {
            0 => (Value::I8(self.u8(at)? as i8), 1),
            1 => (Value::U8(self.u8(at)?), 1),
            2 => (Value::I16(self.u16(at)? as i16), 2),
            3 => (Value::U16(self.u16(at)?), 2),
            4 => (Value::I32(self.u32(at)? as i32), 4),
            5 => (Value::U32(self.u32(at)?), 4),
            6 => (Value::I64(self.u64(at)? as i64), 8),
            7 => (Value::U64(self.u64(at)?), 8),
            8 => (Value::F32(self.f32(at)?), 4),
            9 => (Value::F64(f64::from_le_bytes(self.bytes(at)?)), 8),
            10 => (Value::Bool(self.u8(at)? != 0), 1),
            11 => (Value::String(self.u64(at)?), 8),
            12 => (Value::Path(self.u64(at)?), 8),
            13 => (Value::EntityPtr(self.u64(at)?), 8),
            14 => (Value::Vector3(self.floats(at)?), 16),
            15 => (Value::Vector4(self.floats(at)?), 16),
            16 => (Value::Quat(self.floats(at)?), 16),
            17 => (Value::Matrix3(self.floats(at)?), 36),
            18 => (Value::Matrix4(self.floats(at)?), 64),
            19 => (Value::Color(self.floats(at)?), 16),
            20 => (Value::FilePtr(self.u64(at)?), 8),
            21 => (Value::EntityHandle(self.u64(at)?), 8),
            22 => (
                Value::EntityLink {
                    package: self.u64(at)?,
                    archive: self.u64(at + 8)?,
                    name: self.u64(at + 16)?,
                    handle: self.u64(at + 24)?,
                },
                32,
            ),
            24 => (
                Value::WideVector3 {
                    xyz: self.floats(at)?,
                    a: self.u16(at + 12)?,
                    b: self.u16(at + 14)?,
                },
                16,
            ),
            other => return Err(Fox2Error::UnknownType(other, at)),
        })
    }

    fn property(&self, at: usize) -> Result<(Property, usize)> {
        let name = self.u64(at)?;
        let ty = self.u8(at + 8)?;
        let container = match self.u8(at + 9)? {
            0 => ContainerKind::StaticArray,
            1 => ContainerKind::DynamicArray,
            2 => ContainerKind::StringMap,
            _ => ContainerKind::List,
        };
        let count = self.u16(at + 10)? as usize;
        let size = self.u16(at + 14)? as usize;
        let mut values = Vec::with_capacity(count);
        let mut p = at + 32;
        for _ in 0..count {
            if container == ContainerKind::StringMap {
                let key = self.u64(p)?;
                let (v, n) = self.value(ty, p + 8)?;
                values.push((Some(key), v));
                p = align16(p + 8 + n);
            } else {
                let (v, n) = self.value(ty, p)?;
                values.push((None, v));
                p += n;
            }
        }
        let next = if size >= 32 { at + size } else { align16(p) };
        Ok((
            Property {
                name,
                container,
                values,
            },
            next,
        ))
    }
}

fn align16(x: usize) -> usize {
    (x + 15) & !15
}

impl DataSet {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = Reader { b: data };
        if r.u32(0)? != MAGIC {
            return Err(Fox2Error::BadMagic);
        }
        let entity_count = r.u32(8)? as usize;
        let string_table = r.u32(12)? as usize;
        let mut at = r.u32(16)? as usize;

        let mut entities = Vec::with_capacity(entity_count);
        for _ in 0..entity_count {
            // 0 header size, 2 unknown, 4 pad, 6 "ent\0", 10 address, 14 pad,
            // 18/22 unknown, 26 version, 28 class hash, 36/38 property
            // counts, 40 offset, 44 static size, 48 total size.
            let address = r.u32(at + 10)?;
            let version = r.u16(at + 26)? as i16;
            let class = r.u64(at + 28)?;
            let n_static = r.u16(at + 36)? as usize;
            let n_dynamic = r.u16(at + 38)? as usize;
            let total = r.u32(at + 48)? as usize;
            let mut p = at + 64;
            let read = |n: usize, p: &mut usize| -> Result<Vec<Property>> {
                (0..n)
                    .map(|_| {
                        let (prop, next) = r.property(*p)?;
                        *p = next;
                        Ok(prop)
                    })
                    .collect()
            };
            let static_properties = read(n_static, &mut p)?;
            let dynamic_properties = read(n_dynamic, &mut p)?;
            entities.push(Entity {
                class,
                address,
                version,
                static_properties,
                dynamic_properties,
            });
            at = if total > 0 { at + total } else { align16(p) };
        }

        let mut strings = HashMap::new();
        let mut s = string_table;
        while let Ok(h) = r.u64(s) {
            if h == 0 {
                break;
            }
            let len = r.u32(s + 8)? as usize;
            let bytes = data
                .get(s + 12..s + 12 + len)
                .ok_or(Fox2Error::Truncated(s))?;
            strings.insert(h, String::from_utf8_lossy(bytes).into_owned());
            s += 12 + len;
        }
        Ok(Self { entities, strings })
    }

    /// Literal for a String/Path/FilePtr hash, from this file or `names`.
    pub fn string<'a>(&'a self, hash: u64, names: &'a NameTable) -> Option<&'a str> {
        self.strings
            .get(&hash)
            .map(String::as_str)
            .or_else(|| names.get(hash))
    }

    /// Human-readable dump, one entity per block.
    pub fn to_text(&self, names: &NameTable) -> String {
        let name = |h: u64| -> String {
            self.string(h, names)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{h:#014x}"))
        };
        let empty = fox_hash::str_code64(b"");
        let lit = |h: u64| -> String {
            if h == 0 || h == empty {
                "\"\"".into()
            } else {
                format!("{:?}", name(h))
            }
        };
        let fmt = |v: &Value| -> String {
            match v {
                Value::String(h) | Value::Path(h) | Value::FilePtr(h) => lit(*h),
                Value::EntityPtr(p) | Value::EntityHandle(p) => format!("{p:#x}"),
                Value::EntityLink {
                    package,
                    archive,
                    name: n,
                    handle,
                } => {
                    format!(
                        "link({}, {}, {}, {handle:#x})",
                        lit(*package),
                        lit(*archive),
                        lit(*n)
                    )
                }
                other => format!("{other:?}"),
            }
        };
        let mut out = String::new();
        for e in &self.entities {
            let _ = writeln!(out, "{} @ {:#x} (v{})", name(e.class), e.address, e.version);
            for (kind, props) in [("", &e.static_properties), ("dyn ", &e.dynamic_properties)] {
                for p in props {
                    let vals: Vec<String> = p
                        .values
                        .iter()
                        .map(|(k, v)| match k {
                            Some(k) => format!("{}={}", name(*k), fmt(v)),
                            None => fmt(v),
                        })
                        .collect();
                    let body = if vals.len() == 1 && p.container == ContainerKind::StaticArray {
                        vals[0].clone()
                    } else {
                        format!("[{}]", vals.join(", "))
                    };
                    let _ = writeln!(out, "  {kind}{} = {body}", name(p.name));
                }
            }
        }
        out
    }
}
