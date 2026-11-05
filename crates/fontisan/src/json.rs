//! A minimal JSON value, so rows and documents render without a serde dependency.
//! Used by scan rows, read documents and audit reports (the C ABI payloads).

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    /// Fixed keys, in the given order.
    Obj(Vec<(&'static str, Json)>),
    /// Data-dependent keys (script tags, feature maps), in the given order.
    Map(Vec<(String, Json)>),
}

impl Json {
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Int(i) => out.push_str(&i.to_string()),
            Self::Str(s) => write_str(s, out),
            Self::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Self::Obj(o) => write_members(o.iter().map(|(k, v)| (*k, v)), out),
            Self::Map(o) => write_members(o.iter().map(|(k, v)| (k.as_str(), v)), out),
        }
    }
}

fn write_members<'a>(members: impl Iterator<Item = (&'a str, &'a Json)>, out: &mut String) {
    out.push('{');
    for (i, (k, v)) in members.enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(k, out);
        out.push(':');
        v.write(out);
    }
    out.push('}');
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<bool> for Json {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<&str> for Json {
    fn from(v: &str) -> Self {
        Self::Str(v.to_string())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}
macro_rules! from_int {
    ($($t:ty),+) => {$(
        impl From<$t> for Json {
            fn from(v: $t) -> Self {
                Self::Int(v as i64)
            }
        }
    )+};
}
from_int!(i16, i32, i64, u8, u16, u32, u64, usize);

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Null, Into::into)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Self {
        Self::Arr(v.into_iter().map(Into::into).collect())
    }
}

/// `obj!{ "key" => value, ... }`: an ordered fixed-key object; values are `Into<Json>`.
macro_rules! obj {
    ($($k:literal => $v:expr),* $(,)?) => {
        $crate::json::Json::Obj(vec![$(($k, $crate::json::Json::from($v))),*])
    };
}
pub(crate) use obj;
