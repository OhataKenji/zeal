//! Parse of `--execution_log_binary_file` output.
//!
//! The file is a sequence of `SpawnExec` messages (`spawn.proto`), each
//! prefixed by its length as a varint, with no compression. Only the fields
//! the run detail shows are read; every other field is skipped by its wire
//! type, so no generated proto code is needed:
//!
//! | field | name             | read as                         |
//! | ----- | ---------------- | ------------------------------- |
//! | 5     | `listed_outputs` | `output` if 11 is absent        |
//! | 10    | `mnemonic`       | `mnemonic`                      |
//! | 11    | `actual_outputs` | `output` (`File.path` of first) |
//! | 12    | `runner`         | `runner`                        |
//! | 13    | `cache_hit`      | `cache_hit`                     |
//! | 14    | `status`         | `status`                        |
//! | 18    | `target_label`   | `label`                         |
//!
//! Bazel writes the file at the end of the invocation. A file cut inside the
//! last message (a killed process) yields the messages before it and
//! `truncated = true`.

use std::io::{self, BufRead, ErrorKind, Read};

/// One spawn, in the order Bazel recorded it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Action {
    /// Owning target, e.g. `//pkg:a`.
    pub label: String,
    pub mnemonic: String,
    /// Exec path of the primary output.
    pub output: String,
    /// Raw runner value, e.g. `linux-sandbox` or `disk cache hit`.
    pub runner: String,
    /// `cache_hit` is set, or the runner ends with ` cache hit`.
    pub cache_hit: bool,
    /// Empty on success, e.g. `NON_ZERO_EXIT` on failure.
    pub status: String,
}

#[derive(Debug, Default)]
pub struct ExecLog {
    pub actions: Vec<Action>,
    pub truncated: bool,
}

/// Errors only on a read failure or a message that is not valid protobuf.
pub fn parse<R: BufRead>(mut input: R) -> io::Result<ExecLog> {
    let mut log = ExecLog::default();
    while !input.fill_buf()?.is_empty() {
        let mut msg = Vec::new();
        let complete = match varint(&mut input) {
            Ok(len) => (&mut input).take(len).read_to_end(&mut msg)? as u64 == len,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => false,
            Err(e) => return Err(e),
        };
        if !complete {
            log.truncated = true;
            break;
        }
        log.actions.push(action(&msg)?);
    }
    Ok(log)
}

fn action(msg: &[u8]) -> io::Result<Action> {
    let mut a = Action::default();
    let mut listed = None;
    let mut first_actual = None;
    fields(msg, |field, value| {
        match (field, value) {
            (5, Value::Bytes(b)) if listed.is_none() => listed = Some(b),
            (10, Value::Bytes(b)) => a.mnemonic = text(b),
            (11, Value::Bytes(b)) if first_actual.is_none() => first_actual = Some(b),
            (12, Value::Bytes(b)) => a.runner = text(b),
            (13, Value::Int(v)) => a.cache_hit = v != 0,
            (14, Value::Bytes(b)) => a.status = text(b),
            (18, Value::Bytes(b)) => a.label = text(b),
            _ => {}
        }
        Ok(())
    })?;
    if let Some(file) = first_actual {
        fields(file, |field, value| {
            if let (1, Value::Bytes(b)) = (field, value) {
                a.output = text(b);
            }
            Ok(())
        })?;
    }
    if a.output.is_empty() {
        a.output = listed.map(text).unwrap_or_default();
    }
    a.cache_hit |= a.runner.ends_with(" cache hit");
    Ok(a)
}

enum Value<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

/// Call `f` for each varint and length-delimited field of one message.
/// Fixed-width fields are skipped.
fn fields<'a>(
    mut msg: &'a [u8],
    mut f: impl FnMut(u64, Value<'a>) -> io::Result<()>,
) -> io::Result<()> {
    while !msg.is_empty() {
        let key = varint(&mut msg)?;
        match key & 7 {
            0 => f(key >> 3, Value::Int(varint(&mut msg)?))?,
            1 => {
                take(&mut msg, 8)?;
            }
            2 => {
                let len = varint(&mut msg)?;
                f(key >> 3, Value::Bytes(take(&mut msg, len)?))?
            }
            5 => {
                take(&mut msg, 4)?;
            }
            _ => return Err(invalid("unsupported wire type")),
        }
    }
    Ok(())
}

fn varint(r: &mut impl Read) -> io::Result<u64> {
    let mut v = 0;
    for shift in (0..64).step_by(7) {
        let mut b = [0];
        r.read_exact(&mut b)?;
        v |= u64::from(b[0] & 0x7f) << shift;
        if b[0] < 0x80 {
            return Ok(v);
        }
    }
    Err(invalid("varint too long"))
}

fn take<'a>(msg: &mut &'a [u8], len: u64) -> io::Result<&'a [u8]> {
    let len = usize::try_from(len)
        .ok()
        .filter(|&n| n <= msg.len())
        .ok_or_else(|| invalid("field runs past the message"))?;
    let (head, rest) = msg.split_at(len);
    *msg = rest;
    Ok(head)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, what)
}
