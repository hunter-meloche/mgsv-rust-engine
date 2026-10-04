//! A reader for the data-only Lua tables the game ships (e.g.
//! `EquipParameters.lua`: `this={}TppEquip.ReloadEquipParameters{key={...}}`).
//! Values are numbers, strings, identifiers such as `TppEquip.RC_30101`
//! (kept as text), and nested tables. Not a Lua interpreter.

#[derive(Clone, Debug, PartialEq)]
pub enum LuaValue {
    Number(f64),
    Str(String),
    /// A bare or dotted identifier, e.g. `TppEquip.RC_30101` or `true`.
    Ident(String),
    Table(LuaTable),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LuaTable {
    /// Positional entries, in order.
    pub items: Vec<LuaValue>,
    /// `key = value` entries, in order.
    pub fields: Vec<(String, LuaValue)>,
}

impl LuaValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            LuaValue::Number(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_ident(&self) -> Option<&str> {
        match self {
            LuaValue::Ident(s) | LuaValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_table(&self) -> Option<&LuaTable> {
        match self {
            LuaValue::Table(t) => Some(t),
            _ => None,
        }
    }
}

impl LuaTable {
    pub fn get(&self, key: &str) -> Option<&LuaValue> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Positional rows that are tables (skipping a leading count/flag number,
    /// as in `{0,{...},{...}}`).
    pub fn rows(&self) -> impl Iterator<Item = &LuaTable> {
        self.items.iter().filter_map(LuaValue::as_table)
    }

    /// The row whose first item is the identifier `key` (e.g. `TppEquip.RC_30101`).
    pub fn row(&self, key: &str) -> Option<&LuaTable> {
        self.rows()
            .find(|r| r.items.first().and_then(LuaValue::as_ident) == Some(key))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("lua table syntax error at byte {0}: {1}")]
pub struct LuaError(pub usize, pub &'static str);

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        loop {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
                continue;
            }
            break;
        }
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'_' | b'.'))
        {
            self.i += 1;
        }
        (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn value(&mut self) -> Result<LuaValue, LuaError> {
        self.ws();
        let c = *self
            .s
            .get(self.i)
            .ok_or(LuaError(self.i, "unexpected end"))?;
        match c {
            b'{' => Ok(LuaValue::Table(self.table()?)),
            b'"' | b'\'' => {
                self.i += 1;
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != c {
                    self.i += if self.s[self.i] == b'\\' { 2 } else { 1 };
                }
                let v =
                    String::from_utf8_lossy(&self.s[start..self.i.min(self.s.len())]).into_owned();
                self.i += 1;
                Ok(LuaValue::Str(v))
            }
            b'-' | b'.' | b'0'..=b'9' => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_alphanumeric()
                        || matches!(self.s[self.i], b'.' | b'-' | b'+'))
                {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
                let n = if let Some(h) = t.strip_prefix("0x") {
                    i64::from_str_radix(h, 16).map(|v| v as f64).ok()
                } else {
                    t.parse::<f64>().ok()
                };
                n.map(LuaValue::Number).ok_or(LuaError(start, "bad number"))
            }
            _ => self
                .ident()
                .map(LuaValue::Ident)
                .ok_or(LuaError(self.i, "unexpected character")),
        }
    }

    fn table(&mut self) -> Result<LuaTable, LuaError> {
        self.i += 1; // {
        let mut t = LuaTable::default();
        loop {
            self.ws();
            match self.s.get(self.i) {
                None => return Err(LuaError(self.i, "unterminated table")),
                Some(b'}') => {
                    self.i += 1;
                    return Ok(t);
                }
                Some(b',') | Some(b';') => {
                    self.i += 1;
                    continue;
                }
                _ => {}
            }
            // `name = value`?
            let save = self.i;
            if let Some(name) = self.ident() {
                self.ws();
                if self.s.get(self.i) == Some(&b'=') && self.s.get(self.i + 1) != Some(&b'=') {
                    self.i += 1;
                    let v = self.value()?;
                    t.fields.push((name, v));
                    continue;
                }
            }
            self.i = save;
            let v = self.value()?;
            t.items.push(v);
        }
    }
}

/// Parse the first table literal passed to `call` (e.g.
/// `TppEquip.ReloadEquipParameters`) in a Lua source file.
pub fn parse_call_argument(src: &str, call: &str) -> Result<LuaTable, LuaError> {
    let at = src.find(call).ok_or(LuaError(0, "call not found"))? + call.len();
    let open = src[at..].find('{').ok_or(LuaError(at, "no table"))? + at;
    let mut p = P {
        s: src.as_bytes(),
        i: open,
    };
    p.table()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_equip_style_tables() {
        let src = "this={}TppEquip.Reload{a={0,{TppEquip.X,1,-2.5,.5,\"s\"}},b={{1},{2}}}";
        let t = parse_call_argument(src, "TppEquip.Reload").unwrap();
        let a = t.get("a").unwrap().as_table().unwrap();
        let row = a.row("TppEquip.X").unwrap();
        assert_eq!(row.items[1].as_f64(), Some(1.0));
        assert_eq!(row.items[2].as_f64(), Some(-2.5));
        assert_eq!(row.items[3].as_f64(), Some(0.5));
        assert_eq!(row.items[4].as_ident(), Some("s"));
        assert_eq!(t.get("b").unwrap().as_table().unwrap().rows().count(), 2);
    }
}
