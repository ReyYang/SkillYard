use std::collections::BTreeMap;

/// 仅实现 Blueprint 恢复与本机维护数据需要的 JSON；BTreeMap 保证稳定输出。
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub fn object() -> Self {
        Self::Object(BTreeMap::new())
    }

    pub fn array() -> Self {
        Self::Array(Vec::new())
    }

    pub fn as_object(&self) -> Result<&BTreeMap<String, Json>, String> {
        match self {
            Self::Object(value) => Ok(value),
            _ => Err("期望 JSON object".to_string()),
        }
    }

    pub fn as_object_mut(&mut self) -> Result<&mut BTreeMap<String, Json>, String> {
        match self {
            Self::Object(value) => Ok(value),
            _ => Err("期望 JSON object".to_string()),
        }
    }

    pub fn as_array(&self) -> Result<&Vec<Json>, String> {
        match self {
            Self::Array(value) => Ok(value),
            _ => Err("期望 JSON array".to_string()),
        }
    }

    pub fn as_str(&self) -> Result<&str, String> {
        match self {
            Self::String(value) => Ok(value),
            _ => Err("期望 JSON string".to_string()),
        }
    }

    pub fn as_bool(&self) -> Result<bool, String> {
        match self {
            Self::Bool(value) => Ok(*value),
            _ => Err("期望 JSON boolean".to_string()),
        }
    }

    pub fn as_u64(&self) -> Result<u64, String> {
        match self {
            Self::Number(value) => value.parse::<u64>().map_err(|_| "期望非负整数".to_string()),
            _ => Err("期望 JSON number".to_string()),
        }
    }

    pub fn get(&self, key: &str) -> Result<&Json, String> {
        self.as_object()?
            .get(key)
            .ok_or_else(|| format!("缺少 JSON 字段：{key}"))
    }

    pub fn get_opt(&self, key: &str) -> Option<&Json> {
        self.as_object().ok().and_then(|object| object.get(key))
    }

    pub fn insert(&mut self, key: impl Into<String>, value: Json) -> Result<(), String> {
        self.as_object_mut()?.insert(key.into(), value);
        Ok(())
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Self {
        Self::Number(value.to_string())
    }
}

impl From<usize> for Json {
    fn from(value: usize) -> Self {
        Self::Number(value.to_string())
    }
}

impl From<i32> for Json {
    fn from(value: i32) -> Self {
        Self::Number(value.to_string())
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            index: 0,
        }
    }

    fn error(&self, message: &str) -> String {
        format!("{message}（byte {}）", self.index)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn take(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.index += 1;
        Some(value)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.index += 1;
        }
    }

    fn parse_value(&mut self) -> Result<Json, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Json::Null)
            }
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Json::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Json::Bool(false))
            }
            Some(b'"') => Ok(Json::String(self.parse_string()?)),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(self.error("JSON token 非法")),
            None => Err(self.error("JSON 意外结束")),
        }
    }

    fn literal(&mut self, expected: &[u8]) -> Result<(), String> {
        if self.bytes.get(self.index..self.index + expected.len()) == Some(expected) {
            self.index += expected.len();
            Ok(())
        } else {
            Err(self.error("JSON literal 非法"))
        }
    }

    fn parse_array(&mut self) -> Result<Json, String> {
        self.take();
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.take();
            return Ok(Json::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            match self.take() {
                Some(b',') => self.skip_whitespace(),
                Some(b']') => break,
                _ => return Err(self.error("JSON array 缺少逗号或右括号")),
            }
        }
        Ok(Json::Array(values))
    }

    fn parse_object(&mut self) -> Result<Json, String> {
        self.take();
        let mut values = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.take();
            return Ok(Json::Object(values));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.error("JSON object key 必须是字符串"));
            }
            let key = self.parse_string()?;
            if values.contains_key(&key) {
                return Err(self.error(&format!("JSON object 含重复 key：{key}")));
            }
            self.skip_whitespace();
            if self.take() != Some(b':') {
                return Err(self.error("JSON object 缺少冒号"));
            }
            let value = self.parse_value()?;
            values.insert(key, value);
            self.skip_whitespace();
            match self.take() {
                Some(b',') => self.skip_whitespace(),
                Some(b'}') => break,
                _ => return Err(self.error("JSON object 缺少逗号或右括号")),
            }
        }
        Ok(Json::Object(values))
    }

    fn parse_number(&mut self) -> Result<Json, String> {
        let start = self.index;
        if self.peek() == Some(b'-') {
            self.index += 1;
        }
        match self.peek() {
            Some(b'0') => self.index += 1,
            Some(b'1'..=b'9') => {
                self.index += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.index += 1;
                }
            }
            _ => return Err(self.error("JSON number 非法")),
        }
        if self.peek() == Some(b'.') {
            self.index += 1;
            let fraction_start = self.index;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
            if fraction_start == self.index {
                return Err(self.error("JSON number 小数部分为空"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.index += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.index += 1;
            }
            let exponent_start = self.index;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
            if exponent_start == self.index {
                return Err(self.error("JSON number 指数部分为空"));
            }
        }
        let number = std::str::from_utf8(&self.bytes[start..self.index])
            .map_err(|_| self.error("JSON number 编码非法"))?;
        Ok(Json::Number(number.to_string()))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.take() != Some(b'"') {
            return Err(self.error("JSON string 缺少引号"));
        }
        let mut output = String::new();
        let mut plain_start = self.index;
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| self.error("JSON string 意外结束"))?;
            match byte {
                b'"' => {
                    self.append_utf8(&mut output, plain_start, self.index)?;
                    self.index += 1;
                    return Ok(output);
                }
                b'\\' => {
                    self.append_utf8(&mut output, plain_start, self.index)?;
                    self.index += 1;
                    let escaped = self
                        .take()
                        .ok_or_else(|| self.error("JSON escape 意外结束"))?;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => self.parse_unicode_escape(&mut output)?,
                        _ => return Err(self.error("JSON escape 非法")),
                    }
                    plain_start = self.index;
                }
                0x00..=0x1f => return Err(self.error("JSON string 含未转义控制字符")),
                _ => self.index += 1,
            }
        }
    }

    fn append_utf8(&self, output: &mut String, start: usize, end: usize) -> Result<(), String> {
        let value = std::str::from_utf8(&self.bytes[start..end])
            .map_err(|_| self.error("JSON string 不是有效 UTF-8"))?;
        output.push_str(value);
        Ok(())
    }

    fn parse_hex_quad(&mut self) -> Result<u16, String> {
        let mut value = 0u16;
        for _ in 0..4 {
            value = value
                .checked_mul(16)
                .ok_or_else(|| self.error("Unicode escape 溢出"))?;
            let digit = match self.take() {
                Some(b'0'..=b'9') => self.bytes[self.index - 1] - b'0',
                Some(b'a'..=b'f') => self.bytes[self.index - 1] - b'a' + 10,
                Some(b'A'..=b'F') => self.bytes[self.index - 1] - b'A' + 10,
                _ => return Err(self.error("Unicode escape 非法")),
            };
            value += digit as u16;
        }
        Ok(value)
    }

    fn parse_unicode_escape(&mut self, output: &mut String) -> Result<(), String> {
        let first = self.parse_hex_quad()?;
        let codepoint = if (0xd800..=0xdbff).contains(&first) {
            if self.take() != Some(b'\\') || self.take() != Some(b'u') {
                return Err(self.error("Unicode 高代理项缺少低代理项"));
            }
            let second = self.parse_hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(self.error("Unicode 低代理项非法"));
            }
            0x10000 + (((first as u32 - 0xd800) << 10) | (second as u32 - 0xdc00))
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err(self.error("Unicode 低代理项没有高代理项"));
        } else {
            first as u32
        };
        let character =
            char::from_u32(codepoint).ok_or_else(|| self.error("Unicode codepoint 非法"))?;
        output.push(character);
        Ok(())
    }
}

pub fn parse_json(input: &str) -> Result<Json, String> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.index != parser.bytes.len() {
        return Err(parser.error("JSON 尾部含额外内容"));
    }
    Ok(value)
}

fn escape_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control <= '\u{001f}' => {
                output.push_str(&format!("\\u{:04x}", control as u32))
            }
            other => output.push(other),
        }
    }
    output.push('"');
}

fn render(value: &Json, output: &mut String, depth: usize, pretty: bool) {
    match value {
        Json::Null => output.push_str("null"),
        Json::Bool(true) => output.push_str("true"),
        Json::Bool(false) => output.push_str("false"),
        Json::Number(number) => output.push_str(number),
        Json::String(string) => escape_string(string, output),
        Json::Array(values) => {
            output.push('[');
            if !values.is_empty() {
                for (index, item) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    if pretty {
                        output.push('\n');
                        output.push_str(&"  ".repeat(depth + 1));
                    }
                    render(item, output, depth + 1, pretty);
                }
                if pretty {
                    output.push('\n');
                    output.push_str(&"  ".repeat(depth));
                }
            }
            output.push(']');
        }
        Json::Object(values) => {
            output.push('{');
            if !values.is_empty() {
                for (index, (key, item)) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    if pretty {
                        output.push('\n');
                        output.push_str(&"  ".repeat(depth + 1));
                    }
                    escape_string(key, output);
                    output.push(':');
                    if pretty {
                        output.push(' ');
                    }
                    render(item, output, depth + 1, pretty);
                }
                if pretty {
                    output.push('\n');
                    output.push_str(&"  ".repeat(depth));
                }
            }
            output.push('}');
        }
    }
}

/// Blueprint 机器块与 hash 使用两空格缩进、排序 key、尾随 LF。
pub fn canonical_json(value: &Json) -> String {
    let mut output = String::new();
    render(value, &mut output, 0, true);
    output.push('\n');
    output
}

pub fn compact_json(value: &Json) -> String {
    let mut output = String::new();
    render(value, &mut output, 0, false);
    output
}

const SHA256_INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// 纯 Rust SHA-256，避免首次恢复联网或依赖系统专用命令。
pub fn sha256_bytes(input: &[u8]) -> String {
    let bit_length = (input.len() as u64).wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());

    let mut state = SHA256_INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut schedule = [0u32; 64];
        for (index, word) in chunk.chunks_exact(4).take(16).enumerate() {
            schedule[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(SHA256_K[index])
                .wrapping_add(schedule[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    state.iter().map(|word| format!("{word:08x}")).collect()
}

pub fn sha256_text(input: &str) -> String {
    sha256_bytes(input.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_text("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn duplicate_key_is_rejected() {
        assert!(parse_json("{\"a\":1,\"a\":2}")
            .unwrap_err()
            .contains("重复 key"));
    }

    #[test]
    fn canonical_round_trip_is_stable() {
        let value = parse_json("{\"z\":1,\"a\":[true,\"中文\"]}").unwrap();
        let canonical = canonical_json(&value);
        assert_eq!(canonical_json(&parse_json(&canonical).unwrap()), canonical);
        assert!(canonical.starts_with("{\n  \"a\""));
    }
}
