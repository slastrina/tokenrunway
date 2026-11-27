//! Antigravity conversation databases store protobuf records in gen_metadata and steps.
//! Layout references are linked in the README. Decode usage and the metadata needed to
//! identify and date it. The reader discards any other fields in the selected blobs.

use super::{Batch, Event, Tokens};
use crate::model::Harness;
use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use url::Url;

pub const ID: &str = "antigravity";
const MAX_BLOB: usize = 8 * 1024 * 1024;
pub(crate) const UNKNOWN_MODEL: &str = "antigravity/unknown";

pub fn databases(root: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(root).into_iter().flatten().flatten().filter_map(|entry| {
        let path = entry.path();
        (entry.file_type().ok()?.is_file() && path.extension().is_some_and(|x| x == "db")).then_some(path)
    })
}

pub fn stamp(path: &Path) -> Option<(u64, i64)> {
    let (size, mtime) = super::stat(path)?;
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push("-wal");
    let (wal_size, wal_mtime) = super::stat(Path::new(&sidecar)).filter(|(size, _)| *size > 0).unwrap_or_default();
    Some((size + wal_size, mtime.max(wal_mtime)))
}

#[derive(Clone, Copy)]
enum Field<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Ignored,
}

struct Message<'a>(Vec<(u32, Field<'a>)>);

fn varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Some(value);
        }
    }
    None
}

fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
    let (part, rest) = bytes.split_at_checked(count)?;
    *bytes = rest;
    Some(part)
}

impl<'a> Message<'a> {
    fn parse(mut bytes: &'a [u8]) -> Option<Self> {
        let mut fields = Vec::new();
        while !bytes.is_empty() {
            let key = varint(&mut bytes)?;
            let number = u32::try_from(key >> 3).ok().filter(|n| (1..=0x1fff_ffff).contains(n))?;
            let field = match key & 7 {
                0 => Field::Number(varint(&mut bytes)?),
                1 => {
                    take(&mut bytes, 8)?;
                    Field::Ignored
                }
                2 => {
                    let length = usize::try_from(varint(&mut bytes)?).ok()?;
                    Field::Bytes(take(&mut bytes, length)?)
                }
                5 => {
                    take(&mut bytes, 4)?;
                    Field::Ignored
                }
                _ => return None,
            };
            if number <= 28 {
                if fields.len() == 4096 {
                    return None;
                }
                fields.push((number, field));
            }
        }
        Some(Self(fields))
    }

    fn bytes(&self, number: u32) -> Option<&'a [u8]> {
        self.0.iter().rev().find_map(|(n, field)| match field {
            Field::Bytes(value) if *n == number => Some(*value),
            _ => None,
        })
    }

    fn nested(&self, number: u32) -> Option<Self> {
        Self::parse(self.bytes(number)?)
    }

    fn number(&self, number: u32) -> Option<u64> {
        self.0.iter().rev().find_map(|(n, field)| match field {
            Field::Number(value) if *n == number => Some(*value),
            _ => None,
        })
    }

    fn text(&self, number: u32) -> Option<&'a str> {
        let s = std::str::from_utf8(self.bytes(number)?).ok()?.trim();
        (!s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control)).then_some(s)
    }
}

fn timestamp(message: Message<'_>) -> Option<i64> {
    let seconds = i64::try_from(message.number(1)?).ok()?;
    let nanos = u32::try_from(message.number(2).unwrap_or(0)).ok()?;
    chrono::DateTime::from_timestamp(seconds, nanos)
        .filter(|_| (1..=253_402_300_799).contains(&seconds) && nanos < 1_000_000_000)
        .map(|time| time.timestamp_millis())
}

fn tokens(usage: &Message<'_>) -> Option<Tokens> {
    // Field 1 is a model enum. Field 3, when present, already includes thinking.
    let reasoning = usage.number(9).unwrap_or(0);
    let output = usage.number(3).unwrap_or(0).max(usage.number(10).unwrap_or(0).checked_add(reasoning)?);
    let input = usage.number(2).unwrap_or(0);
    let cache_write_5m = usage.number(4).unwrap_or(0);
    let cache_read = usage.number(5).unwrap_or(0);
    // SQLite's integer storage is signed.
    let total = input.checked_add(output)?.checked_add(cache_read)?.checked_add(cache_write_5m)?;
    (total > 0 && i64::try_from(total).is_ok()).then_some(Tokens {
        input,
        output,
        cache_read,
        cache_write_5m,
        reasoning,
        ..Tokens::default()
    })
}

#[derive(Default)]
struct Steps {
    by_bot: HashMap<(String, String), Option<i64>>,
    by_index: HashMap<i64, (Option<String>, i64)>,
}

impl Steps {
    fn read(conn: &Connection) -> rusqlite::Result<Self> {
        let mut result = Self::default();
        if !has_table(conn, "steps")? {
            return Ok(result);
        }
        let mut statement =
            conn.prepare("SELECT idx, CASE WHEN length(metadata) <= ?1 THEN metadata END FROM steps WHERE metadata IS NOT NULL")?;
        let mut rows = statement.query([MAX_BLOB])?;
        while let Some(row) = rows.next()? {
            let blob: Option<Vec<u8>> = row.get(1)?;
            let Some(metadata) = blob.as_deref().and_then(Message::parse) else { continue };
            let time = metadata.nested(8).and_then(timestamp).or_else(|| metadata.nested(1).and_then(timestamp));
            let step = metadata.text(12);
            let bot = metadata.nested(9).and_then(|usage| usage.text(7));
            if let (Some(step), Some(bot)) = (step, bot) {
                result.by_bot.entry((step.into(), bot.into())).and_modify(|known| *known = None).or_insert(time);
            }
            if let Some(time) = time {
                result.by_index.insert(row.get(0)?, (step.map(str::to_owned), time));
            }
        }
        Ok(result)
    }

    fn time_for(&self, root: &Message<'_>, usage: &Message<'_>) -> Option<i64> {
        let step = root.text(4);
        if let (Some(step), Some(bot)) = (step, usage.text(7)) {
            if let Some(time) = self.by_bot.get(&(step.into(), bot.into())).copied().flatten() {
                return Some(time);
            }
        }
        // Some versions store explicit step indices, including packed indices. Use them
        // only if they agree on a timestamp and belong to this step when a UUID is present.
        let mut times = HashSet::new();
        let mut record = |index: u64| -> Option<()> {
            let (id, ts) = self.by_index.get(&i64::try_from(index).ok()?)?;
            if step.is_some() && id.as_deref() != step {
                return None;
            }
            times.insert(*ts);
            Some(())
        };
        for (_, field) in root.0.iter().filter(|(number, _)| *number == 2) {
            match *field {
                Field::Number(index) => record(index)?,
                Field::Bytes(mut bytes) => {
                    while !bytes.is_empty() {
                        record(varint(&mut bytes)?)?;
                    }
                }
                Field::Ignored => return None,
            }
        }
        (times.len() == 1).then(|| times.into_iter().next()).flatten()
    }
}

fn has_table(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema = 'main' AND name = ?1 AND type = 'table')", [table], |r| {
        r.get(0)
    })
}

fn workspace(conn: &Connection) -> String {
    let blob = conn
        .query_row("SELECT data FROM trajectory_metadata_blob WHERE length(data) <= ?1 LIMIT 1", [MAX_BLOB], |row| row.get::<_, Vec<u8>>(0))
        .ok();
    blob.as_deref()
        .and_then(Message::parse)
        .and_then(|root| root.nested(1))
        .and_then(|folder| folder.text(1))
        .and_then(|uri| Url::parse(uri).ok())
        .and_then(|uri| uri.to_file_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_db(path: &Path) -> rusqlite::Result<Batch> {
    let mut batch = Batch::default();
    let Some(session) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(batch);
    };
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(std::time::Duration::from_millis(250))?;
    conn.pragma_update(None, "trusted_schema", false)?;
    let tx = conn.transaction()?;
    if !has_table(&tx, "gen_metadata")? {
        return Ok(batch);
    }
    let cwd = if has_table(&tx, "trajectory_metadata_blob")? { workspace(&tx) } else { String::new() };
    let mut steps = None;
    let mut models_by_label = HashMap::<String, Option<String>>::new();
    let mut labels = Vec::new();
    let mut statement =
        tx.prepare("SELECT idx, CASE WHEN length(data) <= ?1 THEN data END FROM gen_metadata WHERE data IS NOT NULL ORDER BY idx")?;
    let mut rows = statement.query([MAX_BLOB])?;
    while let Some(row) = rows.next()? {
        let index = row.get::<_, i64>(0)?;
        let blob: Option<Vec<u8>> = row.get(1)?;
        let Some(root) = blob.as_deref().and_then(Message::parse) else { continue };
        let Some(model) = root.nested(1) else { continue };
        let Some(usage) = model.nested(4) else { continue };
        let Some(tokens) = tokens(&usage) else { continue };
        let time = model.nested(9).and_then(|m| m.nested(4)).and_then(timestamp);
        let ts = match time {
            Some(ts) => ts,
            None => {
                if steps.is_none() {
                    steps = Some(Steps::read(&tx)?);
                }
                let Some(ts) = steps.as_ref().and_then(|steps| steps.time_for(&root, &usage)) else { continue };
                ts
            }
        };
        let response = usage.text(11).or_else(|| usage.text(12));
        let key = match response {
            Some(value) => format!("agy:{session}:response:{value}"),
            None => format!("agy:{session}:row:{index}"),
        };
        let model_name = model.text(19).filter(|name| !name.starts_with("MODEL_PLACEHOLDER"));
        let label = model.text(21);
        if let (Some(label), Some(name)) = (label, model_name) {
            models_by_label
                .entry(label.into())
                .and_modify(|known| {
                    if known.as_deref() != Some(name) {
                        *known = None;
                    }
                })
                .or_insert_with(|| Some(name.into()));
        }
        labels.push(label.map(str::to_owned));
        batch.events.push(Event {
            key,
            ts,
            harness: Harness::Antigravity,
            model: model_name.unwrap_or(UNKNOWN_MODEL).into(),
            session_id: session.into(),
            turn_id: None,
            cwd: cwd.clone(),
            branch: None,
            sidechain: false,
            fast: false,
            tokens,
            cost_actual: None,
            account: None,
        });
    }
    // Some continuations report only the display label. Use a model identifier from the
    // same conversation only when that label was associated with exactly one identifier.
    for (event, label) in batch.events.iter_mut().zip(labels) {
        if event.model == UNKNOWN_MODEL {
            if let Some(model) = label.as_deref().and_then(|label| models_by_label.get(label)).and_then(Option::as_ref) {
                event.model.clone_from(model);
            }
        }
    }
    Ok(batch)
}

/// A changed conversation database is reread in one snapshot. Stable event keys deduplicate
/// checkpoints and copies of a session with the same filename.
pub fn read(path: &Path, _offset: u64, _state: &str) -> io::Result<(Batch, u64, String)> {
    read_db(path).map(|batch| (batch, 0, String::new())).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::Roots;
    use crate::stats::Stats;

    fn encode(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        while value >= 128 {
            bytes.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }

    fn number(field: u64, value: u64) -> Vec<u8> {
        [encode(field << 3), encode(value)].concat()
    }

    fn nested(field: u64, data: impl AsRef<[u8]>) -> Vec<u8> {
        let data = data.as_ref();
        [encode((field << 3) | 2).as_slice(), &encode(data.len() as u64), data].concat()
    }

    fn generation(model: Option<&str>, response: &str, time: Option<u64>) -> Vec<u8> {
        let usage = [
            number(1, 1298),
            number(2, 100),
            number(3, 13),
            number(4, 20),
            number(5, 40),
            nested(7, response),
            number(9, 5),
            number(10, 8),
            nested(11, response),
        ]
        .concat();
        let mut chat = [nested(4, usage), nested(21, "Gemini Test"), nested(50, "private-test-prompt")].concat();
        if let Some(time) = time {
            chat.extend(nested(9, nested(4, number(1, time))));
        }
        if let Some(model) = model {
            chat.extend(nested(19, model));
        }
        [nested(1, chat), nested(4, "step-uuid")].concat()
    }

    #[test]
    fn imports_conversations_with_stable_totals_and_includes_wal_updates() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = tmp.path().join("cli");
        let desktop = tmp.path().join("desktop");
        let project = tmp.path().join("sample-project");
        for dir in [&cli, &desktop, &project] {
            std::fs::create_dir(dir).unwrap();
        }
        let path = cli.join("fixture-conversation.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);
             CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE trajectory_metadata_blob (data BLOB);",
        )
        .unwrap();
        let time = 1_795_000_000;
        let uri = Url::from_directory_path(&project).unwrap();
        let trajectory = [nested(1, nested(1, uri.as_str())), nested(2, number(1, time - 100))].concat();
        db.execute("INSERT INTO trajectory_metadata_blob VALUES (?1)", [trajectory]).unwrap();

        let insert = |db: &Connection, index: i64, blob: Vec<u8>| {
            db.execute("INSERT INTO gen_metadata VALUES (?1, ?2)", rusqlite::params![index, blob]).unwrap();
        };
        let start = generation(Some("gemini-2.5-pro"), "first", Some(time));
        insert(&db, 1, start.clone());
        insert(&db, 2, start);
        insert(&db, 3, generation(None, "second", None));
        insert(&db, 4, generation(None, "undated", None));
        insert(&db, 5, vec![0xff]); // malformed protobuf
        let step = [nested(1, number(1, time + 10)), nested(9, nested(7, "second")), nested(12, "step-uuid")].concat();
        db.execute("INSERT INTO steps VALUES (17, ?1)", [step]).unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        std::fs::copy(&path, desktop.join(path.file_name().unwrap())).unwrap();
        Connection::open(cli.join("conversation_summaries.db")).unwrap().execute_batch("CREATE TABLE summaries (title TEXT)").unwrap();

        let stats_db = tmp.path().join("activity.db");
        let roots = Roots { antigravity: vec![cli, desktop], ..Default::default() };
        let stats = Stats::open_with(&stats_db, roots).unwrap();
        let now = (time as i64 + 60) * 1000;
        let report = stats.ingest();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.events_added, 2);
        assert_eq!(stats.ingest().files_read, 0);
        let totals = stats.stats("lifetime", now);
        assert_eq!(totals.tokens.total, 346);
        assert!(totals.spend_api > 0.0);
        assert_eq!(totals.spend_actual, None);
        assert_eq!(stats.harness("lifetime", now).harnesses[0].name, "Antigravity");
        let dashboard = stats.dashboard("lifetime", "harness", now);
        assert_eq!(dashboard.sessions_total, 1);
        assert_eq!(dashboard.sessions[0].project, "sample-project");
        let detail = stats.session(ID, "fixture-conversation").unwrap();
        assert_eq!(detail.calls, 2);
        assert_eq!(detail.last, (time as i64 + 10) * 1000);
        assert_eq!(
            (detail.steps[0].input, detail.steps[0].output, detail.steps[0].cache_read, detail.steps[0].cache_write),
            (100, 13, 40, 20)
        );

        insert(&db, 6, generation(Some("gemini-2.5-pro"), "third", Some(time + 20)));
        let report = stats.ingest();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!((report.files_read, report.events_added), (1, 1));
        assert_eq!(stats.stats("lifetime", now).tokens.total, 519);
        drop(stats);
        let stored = std::fs::read(stats_db).unwrap();
        assert!(!stored.windows(b"private-test-prompt".len()).any(|part| part == b"private-test-prompt"));
    }

    #[test]
    fn recovers_time_from_an_explicit_step_index() {
        let mut steps = Steps::default();
        steps.by_index.insert(32, (Some("known-step".into()), 50));
        let buf = [nested(2, encode(32)), nested(4, "known-step")].concat();
        let root = Message::parse(&buf).unwrap();
        assert_eq!(steps.time_for(&root, &Message::parse(&[]).unwrap()), Some(50));
        let buf = [nested(2, encode(32)), nested(4, "wrong-step")].concat();
        assert_eq!(steps.time_for(&Message::parse(&buf).unwrap(), &Message::parse(&[]).unwrap()), None);
    }
}
