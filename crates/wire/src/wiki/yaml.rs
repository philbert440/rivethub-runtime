use std::fmt::{Display, Write as _};

use serde::Serialize;
use serde::ser::{self, Impossible};

#[derive(Debug, thiserror::Error)]
pub enum EmitError {
    #[error("{0}")]
    Custom(String),
    #[error(transparent)]
    Format(#[from] std::fmt::Error),
    #[error("YAML frontmatter cannot hold {0}")]
    Unsupported(&'static str),
}

impl ser::Error for EmitError {
    fn custom<T: Display>(message: T) -> Self {
        Self::Custom(message.to_string())
    }
}

pub(super) fn to_block_yaml<T: Serialize>(value: &T) -> Result<String, EmitError> {
    let mut out = String::new();
    value.serialize(Node {
        out: &mut out,
        indent: 0,
        place: Place::Document,
    })?;
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Document,
    Key,
    MapValue,
    SeqItem,
}

struct Node<'a> {
    out: &'a mut String,
    indent: usize,
    place: Place,
}

impl<'a> Node<'a> {
    fn scalar(self, text: &str) {
        match self.place {
            Place::Key => self.out.push_str(text),
            Place::MapValue => {
                self.out.push(' ');
                self.out.push_str(text);
                self.out.push('\n');
            }
            Place::SeqItem | Place::Document => {
                self.out.push_str(text);
                self.out.push('\n');
            }
        }
    }

    fn string(self, text: &str) -> Result<(), EmitError> {
        match (scalar_style(text), self.place) {
            (Style::Literal, Place::MapValue) => {
                self.out.push(' ');
                self.out.push_str(&literal(text, self.indent + 2));
            }
            (Style::Literal, Place::SeqItem) => {
                self.out.push_str(&literal(text, self.indent + 2));
            }
            (
                Style::Literal | Style::Double,
                Place::Document | Place::Key | Place::MapValue | Place::SeqItem,
            ) => {
                self.scalar(&double_quoted(text)?);
            }
            (Style::Single, _) => self.scalar(&format!("'{}'", text.replace('\'', "''"))),
            (Style::Plain, _) => self.scalar(text),
        }
        Ok(())
    }

    fn collection(self, kind: Collection) -> Result<Block<'a>, EmitError> {
        if self.place == Place::Key {
            return Err(EmitError::Unsupported("a collection as a key"));
        }
        let (indent, inline) = match self.place {
            Place::Document => (0, true),
            Place::SeqItem => (self.indent + 2, true),
            Place::MapValue | Place::Key => (self.indent + 2, false),
        };
        Ok(Block {
            out: self.out,
            indent,
            inline,
            place: self.place,
            kind,
            entries: 0,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collection {
    Seq,
    Map,
}

struct Block<'a> {
    out: &'a mut String,
    indent: usize,
    inline: bool,
    place: Place,
    kind: Collection,
    entries: usize,
}

impl Block<'_> {
    fn open_entry(&mut self) {
        if self.entries == 0 && self.place == Place::MapValue {
            self.out.push('\n');
        }
        if self.entries > 0 || !self.inline {
            self.out.push_str(&" ".repeat(self.indent));
        }
        if self.kind == Collection::Seq {
            self.out.push_str("- ");
        }
        self.entries += 1;
    }

    fn item<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EmitError> {
        self.open_entry();
        value.serialize(Node {
            out: self.out,
            indent: self.indent,
            place: Place::SeqItem,
        })
    }

    fn key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), EmitError> {
        self.open_entry();
        key.serialize(Node {
            out: self.out,
            indent: self.indent,
            place: Place::Key,
        })?;
        self.out.push(':');
        Ok(())
    }

    fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EmitError> {
        value.serialize(Node {
            out: self.out,
            indent: self.indent,
            place: Place::MapValue,
        })
    }

    fn close(self) {
        if self.entries == 0 {
            let empty = if self.kind == Collection::Seq {
                "[]"
            } else {
                "{}"
            };
            match self.place {
                Place::MapValue => self.out.push(' '),
                Place::Document | Place::SeqItem | Place::Key => {}
            }
            self.out.push_str(empty);
            self.out.push('\n');
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    Single,
    Double,
    Literal,
}

fn scalar_style(text: &str) -> Style {
    if text.contains('\n') {
        return Style::Literal;
    }
    if plain_safe(text) && !resolves_to_other_type(text) {
        return Style::Plain;
    }
    if text.contains('"') && !text.contains('\'') {
        Style::Single
    } else {
        Style::Double
    }
}

fn plain_safe(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    let indicator = matches!(
        first,
        '\t' | ' '
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
    );
    let lone_indicator = matches!(text, "-" | "?")
        || ((text.starts_with('-') || text.starts_with('?')) && text[1..].starts_with([' ', '\t']));
    let inner = text.contains(": ")
        || text.contains(":\t")
        || text.contains(" #")
        || text.contains("\t#")
        || text.ends_with([' ', '\t', ':']);
    let marker = text.starts_with("---") || text.starts_with("...");
    !(indicator || lone_indicator || inner || marker)
}

fn resolves_to_other_type(text: &str) -> bool {
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    matches!(
        text,
        "" | "~"
            | "null"
            | "Null"
            | "NULL"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) || matches!(unsigned, ".inf" | ".Inf" | ".INF")
        || is_decimal(unsigned)
        || is_radix(text, "0o", |ch| ch.is_digit(8))
        || is_radix(text, "0x", |ch| ch.is_ascii_hexdigit())
}

fn is_radix(text: &str, prefix: &str, digit: impl Fn(char) -> bool) -> bool {
    text.strip_prefix(prefix)
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(digit))
}

fn is_decimal(text: &str) -> bool {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(index) => (&text[..index], Some(&text[index + 1..])),
        None => (text, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    let digits = |part: &str| part.chars().all(|ch| ch.is_ascii_digit());
    let mantissa_ok = match fraction {
        Some(fraction) => {
            digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty())
        }
        None => !whole.is_empty() && digits(whole),
    };
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    mantissa_ok && exponent_ok
}

fn double_quoted(text: &str) -> Result<String, EmitError> {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            ch if ch.is_control() => write!(quoted, "\\u{:04x}", u32::from(ch))?,
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

fn literal(text: &str, indent: usize) -> String {
    let chomp = if !text.ends_with('\n') {
        "-"
    } else if text.ends_with("\n\n") {
        "+"
    } else {
        ""
    };
    let indicator = if text.starts_with([' ', '\n']) {
        "2"
    } else {
        ""
    };
    let pad = " ".repeat(indent);
    let body = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<String> = body
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect();
    format!("|{indicator}{chomp}\n{}\n", lines.join("\n"))
}

impl<'a> ser::Serializer for Node<'a> {
    type Ok = ();
    type Error = EmitError;
    type SerializeSeq = Block<'a>;
    type SerializeTuple = Block<'a>;
    type SerializeTupleStruct = Block<'a>;
    type SerializeTupleVariant = Impossible<(), EmitError>;
    type SerializeMap = Block<'a>;
    type SerializeStruct = Block<'a>;
    type SerializeStructVariant = Impossible<(), EmitError>;

    fn serialize_bool(self, value: bool) -> Result<(), EmitError> {
        self.scalar(if value { "true" } else { "false" });
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<(), EmitError> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<(), EmitError> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<(), EmitError> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i64(self, value: i64) -> Result<(), EmitError> {
        self.scalar(&value.to_string());
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> Result<(), EmitError> {
        self.serialize_u64(u64::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<(), EmitError> {
        self.serialize_u64(u64::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<(), EmitError> {
        self.serialize_u64(u64::from(value))
    }

    fn serialize_u64(self, value: u64) -> Result<(), EmitError> {
        self.scalar(&value.to_string());
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> Result<(), EmitError> {
        self.serialize_f64(f64::from(value))
    }

    fn serialize_f64(self, value: f64) -> Result<(), EmitError> {
        let text = if value.is_nan() {
            ".nan".to_owned()
        } else if value.is_infinite() {
            if value > 0.0 { ".inf" } else { "-.inf" }.to_owned()
        } else {
            value.to_string()
        };
        self.scalar(&text);
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), EmitError> {
        self.string(value.encode_utf8(&mut [0; 4]))
    }

    fn serialize_str(self, value: &str) -> Result<(), EmitError> {
        self.string(value)
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<(), EmitError> {
        Err(EmitError::Unsupported("bytes"))
    }

    fn serialize_none(self) -> Result<(), EmitError> {
        self.scalar("null");
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), EmitError> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), EmitError> {
        self.scalar("null");
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), EmitError> {
        self.scalar("null");
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<(), EmitError> {
        self.string(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), EmitError> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<(), EmitError> {
        Err(EmitError::Unsupported("an enum variant with data"))
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Block<'a>, EmitError> {
        self.collection(Collection::Seq)
    }

    fn serialize_tuple(self, _: usize) -> Result<Block<'a>, EmitError> {
        self.collection(Collection::Seq)
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Block<'a>, EmitError> {
        self.collection(Collection::Seq)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, EmitError> {
        Err(EmitError::Unsupported("an enum variant with data"))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Block<'a>, EmitError> {
        self.collection(Collection::Map)
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Block<'a>, EmitError> {
        self.collection(Collection::Map)
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, EmitError> {
        Err(EmitError::Unsupported("an enum variant with data"))
    }
}

impl ser::SerializeSeq for Block<'_> {
    type Ok = ();
    type Error = EmitError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EmitError> {
        self.item(value)
    }

    fn end(self) -> Result<(), EmitError> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeTuple for Block<'_> {
    type Ok = ();
    type Error = EmitError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EmitError> {
        self.item(value)
    }

    fn end(self) -> Result<(), EmitError> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeTupleStruct for Block<'_> {
    type Ok = ();
    type Error = EmitError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EmitError> {
        self.item(value)
    }

    fn end(self) -> Result<(), EmitError> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeMap for Block<'_> {
    type Ok = ();
    type Error = EmitError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), EmitError> {
        self.key(key)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EmitError> {
        self.value(value)
    }

    fn end(self) -> Result<(), EmitError> {
        self.close();
        Ok(())
    }
}

impl ser::SerializeStruct for Block<'_> {
    type Ok = ();
    type Error = EmitError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), EmitError> {
        self.key(key)?;
        self.value(value)
    }

    fn end(self) -> Result<(), EmitError> {
        self.close();
        Ok(())
    }
}
