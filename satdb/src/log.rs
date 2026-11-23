//! The durable transaction log behind `Conn::open`: an append-only file with one record per committed transaction.
//!
//! # Format
//! The file starts with an 8-byte magic and a 4-byte format version. Then each record is a 12-byte header and a payload:
//! the payload's length (u32), a CRC-32 of those 4 length bytes, a CRC-32 of the payload, and the payload. All integers are little-endian.
//! The payload is a `TxRecord`, the *effect* of the transaction (its basis, its entity, the next entity id after it, its instant,
//! the attributes it defined with their ids, and the datoms it retracted and asserted), so replaying it doesn't resolve tempids,
//! lookup refs or upserts again; `Db::replay` checks that it fits the database before it.
//!
//! # Commit and recovery
//! `append` writes a record at the end of the committed data and syncs it (`File::sync_data`) before `Conn::transact` returns or
//! makes the transaction visible. If the write or the sync fails, the file is truncated back to the committed data; if even that fails,
//! the log refuses every later append (the file's end is unknown), and reopening it recovers.
//!
//! Opening scans the records from the start. With `rest` bytes left at a record's offset:
//! * fewer than a header, or a valid header whose payload runs past the end of the file: a *torn* final record, an append that was
//!   interrupted and never acknowledged, which is truncated away (as is a file shorter than its header that is a prefix of it);
//! * a header whose length checksum is wrong, a payload whose checksum is wrong, a payload that doesn't decode, or a record that
//!   `Db::replay` refuses: `Error::CorruptLog` at the record's offset, and the file isn't modified. The length's own checksum is what
//!   tells a corrupted length (which may point past the end of the file) from a torn record.
//!
//! A complete final record with a wrong payload checksum is corruption too, not a torn write: every acknowledged record was synced,
//! so reporting it is safer than silently dropping a transaction that may have been acknowledged.
//!
//! The file is locked (`File::try_lock`) while a connection holds it, so a second connection can't interleave appends.
//!
//! # Checkpoints
//! `Conn::checkpoint` writes `<log>.ckpt`: a 36-byte header (magic, format version, the length of the log prefix it covers, the CRC-32
//! of that prefix, the payload's length and CRC-32) and the payload, a `Snapshot` of the database after the covered records (what can't be
//! derived: the counters, the schema history, the transactions, and the history in log order). It is written to a temporary file, synced,
//! and renamed over the previous one. Opening uses it when it is intact (both checksums), its covered prefix ends at a record boundary of
//! the log and is the log's (the prefix's CRC), and its basis is the number of records it covers; then `Db::from_snapshot` rebuilds every
//! index from the history in O(n log n), and only the later records are replayed. In every other case the checkpoint is ignored and the log
//! replayed in full: the log holds the data, the checkpoint is a cache.

use crate::db::{Attr, AttrId, Cardinality, Db, Eid, Error, Snapshot, TxRecord, Unique, Value, ValueType};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"satdblog";
const CKPT_MAGIC: &[u8; 8] = b"satdbckp";
const VERSION: u32 = 1;
const FILE_HEADER: usize = 12;
const RECORD_HEADER: usize = 12;
const CKPT_HEADER: usize = 36;

/// The CRC-32 (IEEE 802.3, reflected) lookup table
const CRC_TABLE: [u32; 256] = {
	let mut table = [0u32; 256];
	let mut i = 0;
	while i < 256
	{
		let mut c = i as u32;
		let mut k = 0;
		while k < 8
		{
			c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
			k += 1;
		}
		table[i] = c;
		i += 1;
	}
	table
};

pub(crate) fn crc32(bytes: &[u8]) -> u32
{
	!bytes.iter().fold(!0u32, |c, &b| CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

fn file_header() -> [u8; FILE_HEADER]
{
	let mut h = [0u8; FILE_HEADER];
	h[..8].copy_from_slice(MAGIC);
	h[8..].copy_from_slice(&VERSION.to_le_bytes());
	h
}

fn io(e: std::io::Error) -> Error
{
	Error::Io(e.to_string())
}

// Encoding

fn put_str(out: &mut Vec<u8>, s: &str)
{
	out.extend_from_slice(&(s.len() as u32).to_le_bytes());
	out.extend_from_slice(s.as_bytes());
}

fn put_value(out: &mut Vec<u8>, v: &Value)
{
	match v
	{
		Value::Bool(b) => out.extend_from_slice(&[0, *b as u8]),
		Value::Int(i) =>
		{
			out.push(1);
			out.extend_from_slice(&i.to_le_bytes());
		}
		Value::Str(s) =>
		{
			out.push(2);
			put_str(out, s);
		}
		Value::Ref(r) =>
		{
			out.push(3);
			out.extend_from_slice(&r.to_le_bytes());
		}
	}
}

fn put_datoms(out: &mut Vec<u8>, ds: &[(Eid, AttrId, Value)])
{
	out.extend_from_slice(&(ds.len() as u32).to_le_bytes());
	for (e, a, v) in ds
	{
		out.extend_from_slice(&e.to_le_bytes());
		out.extend_from_slice(&a.to_le_bytes());
		put_value(out, v);
	}
}

/// The payload of a record
pub(crate) fn encode(rec: &TxRecord) -> Vec<u8>
{
	let mut out = Vec::new();
	for x in [rec.basis_t, rec.tx, rec.next_eid]
	{
		out.extend_from_slice(&x.to_le_bytes());
	}
	out.extend_from_slice(&rec.instant.to_le_bytes());
	out.extend_from_slice(&(rec.attrs.len() as u32).to_le_bytes());
	for (a, def) in &rec.attrs
	{
		out.extend_from_slice(&a.to_le_bytes());
		put_attr(&mut out, def);
	}
	put_datoms(&mut out, &rec.retracted);
	put_datoms(&mut out, &rec.asserted);
	out
}

fn put_attr(out: &mut Vec<u8>, def: &Attr)
{
	put_str(out, &def.name);
	let vt = match def.value_type
	{
		ValueType::Bool => 0,
		ValueType::Int => 1,
		ValueType::Str => 2,
		ValueType::Ref => 3,
	};
	let unique = match def.unique
	{
		None => 0,
		Some(Unique::Identity) => 1,
		Some(Unique::Value) => 2,
	};
	out.extend_from_slice(&[vt, (def.cardinality == Cardinality::Many) as u8, unique, def.component as u8]);
}

/// The payload of a checkpoint
fn encode_snapshot(s: &Snapshot) -> Vec<u8>
{
	let mut out = Vec::new();
	for x in [s.basis_t, s.next_eid, s.instant as u64, s.schema.len() as u64, s.txs.len() as u64, s.history.len() as u64]
	{
		out.extend_from_slice(&x.to_le_bytes());
	}
	for ((a, t), def) in &s.schema
	{
		out.extend_from_slice(&a.to_le_bytes());
		out.extend_from_slice(&t.to_le_bytes());
		put_attr(&mut out, def);
	}
	for (t, tx) in &s.txs
	{
		out.extend_from_slice(&t.to_le_bytes());
		out.extend_from_slice(&tx.to_le_bytes());
	}
	for (tx, e, a, v, added) in &s.history
	{
		out.extend_from_slice(&tx.to_le_bytes());
		out.extend_from_slice(&e.to_le_bytes());
		out.extend_from_slice(&a.to_le_bytes());
		put_value(&mut out, v);
		out.push(*added as u8);
	}
	out
}

/// A record as it is written: the header (length, its checksum, the payload's checksum), then the payload
pub(crate) fn frame(payload: &[u8]) -> Result<Vec<u8>, Error>
{
	let len = u32::try_from(payload.len()).map_err(|_| Error::Io(format!("a transaction of {} bytes is too large for the log", payload.len())))?.to_le_bytes();
	let mut out = Vec::with_capacity(RECORD_HEADER + payload.len());
	out.extend_from_slice(&len);
	out.extend_from_slice(&crc32(&len).to_le_bytes());
	out.extend_from_slice(&crc32(payload).to_le_bytes());
	out.extend_from_slice(payload);
	Ok(out)
}

// Decoding: total over arbitrary bytes (every read is bounds-checked, every tag and string validated)

struct Reader<'a>
{
	bytes: &'a [u8],
	pos: usize,
}

impl<'a> Reader<'a>
{
	fn take(&mut self, n: usize) -> Result<&'a [u8], String>
	{
		if self.bytes.len() - self.pos < n
		{
			return Err(format!("the payload ends at byte {} of a field of {n} bytes", self.pos));
		}
		self.pos += n;
		Ok(&self.bytes[self.pos - n..self.pos])
	}

	fn u8(&mut self) -> Result<u8, String>
	{
		Ok(self.take(1)?[0])
	}

	fn u32(&mut self) -> Result<u32, String>
	{
		Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
	}

	fn u64(&mut self) -> Result<u64, String>
	{
		Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
	}

	fn str(&mut self) -> Result<&'a str, String>
	{
		let n = self.u32()? as usize;
		std::str::from_utf8(self.take(n)?).map_err(|e| format!("a string that isn't UTF-8: {e}"))
	}

	fn tag(&mut self, what: &str, max: u8) -> Result<u8, String>
	{
		match self.u8()?
		{
			t if t <= max => Ok(t),
			t => Err(format!("{what} {t}")),
		}
	}

	fn value(&mut self) -> Result<Value, String>
	{
		Ok(match self.tag("value tag", 3)?
		{
			0 => Value::Bool(self.tag("boolean", 1)? == 1),
			1 => Value::Int(self.u64()? as i64),
			2 => Value::from(self.str()?),
			_ => Value::Ref(self.u64()?),
		})
	}

	fn attr(&mut self) -> Result<Attr, String>
	{
		let name = self.str()?;
		let value_type = [ValueType::Bool, ValueType::Int, ValueType::Str, ValueType::Ref][self.tag("value type", 3)? as usize];
		let cardinality = [Cardinality::One, Cardinality::Many][self.tag("cardinality", 1)? as usize];
		let unique = [None, Some(Unique::Identity), Some(Unique::Value)][self.tag("uniqueness", 2)? as usize];
		let component = self.tag("component flag", 1)? == 1;
		Ok(Attr { name: name.into(), value_type, cardinality, unique, component })
	}

	fn datoms(&mut self) -> Result<Vec<(Eid, AttrId, Value)>, String>
	{
		// The count isn't trusted for an allocation: a corrupt one fails on the first missing datom
		let n = self.u32()?;
		let mut out = Vec::new();
		for _ in 0..n
		{
			out.push((self.u64()?, self.u32()?, self.value()?));
		}
		Ok(out)
	}
}

/// The record of a payload (an error, never a panic, for any bytes)
pub(crate) fn decode(payload: &[u8]) -> Result<TxRecord, String>
{
	let mut r = Reader { bytes: payload, pos: 0 };
	let (basis_t, tx, next_eid, instant) = (r.u64()?, r.u64()?, r.u64()?, r.u64()? as i64);
	let mut attrs = Vec::new();
	for _ in 0..r.u32()?
	{
		let a = r.u32()?;
		attrs.push((a, r.attr()?));
	}
	let (retracted, asserted) = (r.datoms()?, r.datoms()?);
	if r.pos != payload.len()
	{
		return Err(format!("{} bytes after the record", payload.len() - r.pos));
	}
	Ok(TxRecord { basis_t, tx, next_eid, instant, attrs, retracted, asserted })
}

/// The snapshot of a checkpoint's payload (an error, never a panic, for any bytes)
fn decode_snapshot(payload: &[u8]) -> Result<Snapshot, String>
{
	let mut r = Reader { bytes: payload, pos: 0 };
	let (basis_t, next_eid, instant) = (r.u64()?, r.u64()?, r.u64()? as i64);
	let (nschema, ntxs, nhistory) = (r.u64()?, r.u64()?, r.u64()?);
	// The counts aren't trusted for allocations: a corrupt one fails on the first missing entry
	let mut s = Snapshot { basis_t, next_eid, instant, schema: Vec::new(), txs: Vec::new(), history: Vec::new() };
	for _ in 0..nschema
	{
		let key = (r.u32()?, r.u64()?);
		s.schema.push((key, r.attr()?));
	}
	for _ in 0..ntxs
	{
		s.txs.push((r.u64()?, r.u64()?));
	}
	for _ in 0..nhistory
	{
		s.history.push((r.u64()?, r.u64()?, r.u32()?, r.value()?, r.tag("added flag", 1)? == 1));
	}
	if r.pos != payload.len()
	{
		return Err(format!("{} bytes after the snapshot", payload.len() - r.pos));
	}
	Ok(s)
}

/// The checkpoint file of the log at `path`: its name with `.ckpt` appended
pub(crate) fn checkpoint_path(path: &Path) -> PathBuf
{
	let mut name = path.file_name().unwrap_or_default().to_os_string();
	name.push(".ckpt");
	path.with_file_name(name)
}

/// A checkpoint file: the header (magic, format version, the length of the log prefix it covers, the CRC-32 of that prefix,
/// the payload's length and CRC-32), then the payload, the snapshot of the database after the covered records
fn checkpoint_bytes(snapshot: &Snapshot, log_prefix: &[u8]) -> Vec<u8>
{
	let payload = encode_snapshot(snapshot);
	let mut out = Vec::with_capacity(CKPT_HEADER + payload.len());
	out.extend_from_slice(CKPT_MAGIC);
	out.extend_from_slice(&VERSION.to_le_bytes());
	out.extend_from_slice(&(log_prefix.len() as u64).to_le_bytes());
	out.extend_from_slice(&crc32(log_prefix).to_le_bytes());
	out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
	out.extend_from_slice(&crc32(&payload).to_le_bytes());
	out.extend_from_slice(&payload);
	out
}

/// The database of a checkpoint file and the number of log records it covers, if it is intact and belongs to the log `bytes`
/// (whose records start at `starts`, and whose complete records end at `end`); None otherwise, so that the log is replayed in full
fn load_checkpoint(ckpt: &[u8], bytes: &[u8], starts: &[usize], end: usize) -> Option<(Db, usize)>
{
	if ckpt.len() < CKPT_HEADER || ckpt[..8] != *CKPT_MAGIC || ckpt[8..12] != VERSION.to_le_bytes()
	{
		return None;
	}
	let u64_at = |i: usize| u64::from_le_bytes(ckpt[i..i + 8].try_into().expect("8 bytes"));
	let u32_at = |i: usize| u32::from_le_bytes(ckpt[i..i + 4].try_into().expect("4 bytes"));
	let (covered, log_crc, len, payload_crc) = (u64_at(12) as usize, u32_at(20), u64_at(24), u32_at(32));
	let payload = &ckpt[CKPT_HEADER..];
	if payload.len() as u64 != len || crc32(payload) != payload_crc
	{
		return None;
	}
	// The covered prefix must end at a record boundary of this log, and be this log's
	let records = if covered == end { starts.len() } else { starts.iter().position(|&s| s == covered)? };
	if crc32(bytes.get(..covered)?) != log_crc
	{
		return None;
	}
	let db = Db::from_snapshot(decode_snapshot(payload).ok()?).ok()?;
	(db.basis_t() == records as u64).then_some((db, records))
}

/// The records of a log file's contents after its header, as (offset, payload), and the end of the last complete record
/// (before a torn final record, if any)
pub(crate) fn records(bytes: &[u8]) -> Result<(Vec<(usize, &[u8])>, usize), Error>
{
	let mut out = Vec::new();
	let mut at = FILE_HEADER;
	let corrupt = |offset: usize, msg: &str| Error::CorruptLog { offset: offset as u64, msg: msg.to_string() };
	while at < bytes.len()
	{
		let rest = &bytes[at..];
		if rest.len() < RECORD_HEADER
		{
			break;
		}
		let word = |i: usize| u32::from_le_bytes(rest[i..i + 4].try_into().expect("4 bytes"));
		if crc32(&rest[..4]) != word(4)
		{
			return Err(corrupt(at, "the record's length fails its checksum"));
		}
		let len = word(0) as usize;
		if rest.len() - RECORD_HEADER < len
		{
			break;
		}
		let payload = &rest[RECORD_HEADER..RECORD_HEADER + len];
		if crc32(payload) != word(8)
		{
			return Err(corrupt(at, "the record fails its checksum"));
		}
		out.push((at, payload));
		at += RECORD_HEADER + len;
	}
	Ok((out, at))
}

/// The database of a log file's contents (its past values are `as_of` views of it), where the valid data ends
/// (`FILE_HEADER` at least, as a torn header is rewritten), and whether the checkpoint `ckpt` was used.
/// An intact checkpoint of this log saves replaying the records it covers; any other checkpoint is ignored, and the log replayed in full.
/// (Should the records after a checkpoint not fit it, the log is replayed in full too: the log is what holds the data, the checkpoint a cache.)
pub(crate) fn replay(bytes: &[u8], ckpt: Option<&[u8]>) -> Result<(Db, usize, bool), Error>
{
	let header = file_header();
	if bytes.len() < FILE_HEADER
	{
		return if header.starts_with(bytes) { Ok((Db::new(), FILE_HEADER, false)) } else { Err(Error::CorruptLog { offset: 0, msg: "not a satdb log".into() }) };
	}
	if bytes[..8] != *MAGIC
	{
		return Err(Error::CorruptLog { offset: 0, msg: "not a satdb log".into() });
	}
	if bytes[8..FILE_HEADER] != header[8..]
	{
		let v = u32::from_le_bytes(bytes[8..FILE_HEADER].try_into().expect("4 bytes"));
		return Err(Error::CorruptLog { offset: 8, msg: format!("format version {v}, this library reads version {VERSION}") });
	}
	let (recs, end) = records(bytes)?;
	let apply = |mut db: Db, recs: &[(usize, &[u8])]| -> Result<Db, Error>
	{
		for &(at, payload) in recs
		{
			let corrupt = |msg: String| Error::CorruptLog { offset: at as u64, msg };
			let rec = decode(payload).map_err(corrupt)?;
			db = db.replay(&rec).map_err(corrupt)?;
		}
		Ok(db)
	};
	let starts: Vec<usize> = recs.iter().map(|r| r.0).collect();
	if let Some((db, covered)) = ckpt.and_then(|c| load_checkpoint(c, bytes, &starts, end))
	{
		if let Ok(db) = apply(db, &recs[covered..])
		{
			return Ok((db, end, true));
		}
	}
	Ok((apply(Db::new(), &recs)?, end, false))
}

/// An open log: the locked file and the length of its committed data
pub(crate) struct Log
{
	file: File,
	len: u64,
	/// A failed append whose truncation failed too: the end of the file is unknown, so nothing more is appended
	broken: bool,
	/// The checkpoint file (see `checkpoint_path`)
	ckpt: PathBuf,
	/// Whether opening started from the checkpoint
	#[cfg(test)]
	pub(crate) from_checkpoint: bool,
	/// Tests inject I/O errors: 1 fails appends after writing half of the record, 2 also fails the truncation that follows
	#[cfg(test)]
	pub(crate) fault: u8,
}

impl Log
{
	/// Open (or create) the log at `path` and replay it: the database after its last record
	pub(crate) fn open(path: &Path) -> Result<(Log, Db), Error>
	{
		let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(io)?;
		file.try_lock().map_err(|e| match e
		{
			std::fs::TryLockError::WouldBlock => Error::Io(format!("{} is held by another connection", path.display())),
			std::fs::TryLockError::Error(e) => io(e),
		})?;
		let mut bytes = Vec::new();
		file.read_to_end(&mut bytes).map_err(io)?;
		// A missing or unreadable checkpoint is just not used
		let ckpt = checkpoint_path(path);
		let ckpt_bytes = std::fs::read(&ckpt).ok();
		#[cfg_attr(not(test), allow(unused_variables))]
		let (db, end, from_checkpoint) = replay(&bytes, ckpt_bytes.as_deref())?;
		if bytes.len() < FILE_HEADER
		{
			// A new file (or a torn header): write the header, and make the file's directory entry durable
			file.set_len(0).map_err(io)?;
			file.seek(SeekFrom::Start(0)).map_err(io)?;
			file.write_all(&file_header()).map_err(io)?;
			file.sync_all().map_err(io)?;
			#[cfg(unix)]
			if let Some(dir) = path.parent()
			{
				let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
				File::open(dir).and_then(|d| d.sync_all()).map_err(io)?;
			}
		}
		else if end < bytes.len()
		{
			// A torn final record
			file.set_len(end as u64).map_err(io)?;
			file.sync_all().map_err(io)?;
		}
		let log = Log {
			file,
			len: end as u64,
			broken: false,
			ckpt,
			#[cfg(test)]
			from_checkpoint,
			#[cfg(test)]
			fault: 0,
		};
		Ok((log, db))
	}

	/// Write a checkpoint of `db`, the database after every committed record: to a temporary file, synced, then renamed over the
	/// previous checkpoint (and the directory synced), so a crash leaves either checkpoint whole. It is a cache: the log keeps every record
	pub(crate) fn checkpoint(&mut self, db: &Db) -> Result<(), Error>
	{
		if self.broken
		{
			return Err(Error::Io("an earlier append failed and the log couldn't be truncated back; reopen it".into()));
		}
		let mut prefix = vec![0; self.len as usize];
		self.file.seek(SeekFrom::Start(0)).map_err(io)?;
		self.file.read_exact(&mut prefix).map_err(io)?;
		let bytes = checkpoint_bytes(&db.snapshot(), &prefix);
		let mut tmp = self.ckpt.clone().into_os_string();
		tmp.push(".tmp");
		let tmp = PathBuf::from(tmp);
		let mut f = File::create(&tmp).map_err(io)?;
		f.write_all(&bytes).and_then(|_| f.sync_all()).map_err(io)?;
		std::fs::rename(&tmp, &self.ckpt).map_err(io)?;
		#[cfg(unix)]
		if let Some(dir) = self.ckpt.parent()
		{
			let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
			File::open(dir).and_then(|d| d.sync_all()).map_err(io)?;
		}
		Ok(())
	}

	/// Append a committed transaction's record and sync it; on an error, the file is truncated back to the committed data
	pub(crate) fn append(&mut self, rec: &TxRecord) -> Result<(), Error>
	{
		if self.broken
		{
			return Err(Error::Io("an earlier append failed and the log couldn't be truncated back; reopen it".into()));
		}
		let bytes = frame(&encode(rec))?;
		if let Err(e) = self.write(&bytes)
		{
			self.broken = self.truncate().is_err();
			return Err(io(e));
		}
		self.len += bytes.len() as u64;
		Ok(())
	}

	fn write(&mut self, bytes: &[u8]) -> std::io::Result<()>
	{
		self.file.seek(SeekFrom::Start(self.len))?;
		#[cfg(test)]
		if self.fault != 0
		{
			self.file.write_all(&bytes[..bytes.len() / 2])?;
			return Err(std::io::Error::other("injected fault"));
		}
		self.file.write_all(bytes)?;
		self.file.sync_data()
	}

	// Cut the file back to the committed data
	fn truncate(&mut self) -> std::io::Result<()>
	{
		#[cfg(test)]
		if self.fault == 2
		{
			return Err(std::io::Error::other("injected fault"));
		}
		self.file.set_len(self.len)?;
		self.file.sync_data()
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::db::tests::{assert_same_db, assert_same_reads, random_workload};
	use crate::db::{Conn, Entity, Tx, TxReport, TX_INSTANT};
	use crate::testutil::Rng;
	use std::fs;
	use std::path::PathBuf;

	/// A scratch directory of one test under target/, emptied first (a crashed run may have left files) and removed when dropped
	struct Scratch(PathBuf);

	impl Scratch
	{
		fn new(name: &str) -> Self
		{
			let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/satdb-log-tests")).join(name);
			let _ = fs::remove_dir_all(&dir);
			fs::create_dir_all(&dir).unwrap();
			Scratch(dir)
		}

		fn path(&self, file: &str) -> PathBuf
		{
			self.0.join(file)
		}
	}

	impl Drop for Scratch
	{
		fn drop(&mut self)
		{
			let _ = fs::remove_dir_all(&self.0);
			// The parent too, once the last test's directory is gone (it fails harmlessly while others remain)
			if let Some(parent) = self.0.parent()
			{
				let _ = fs::remove_dir(parent);
			}
		}
	}

	/// More transactions after `random_workload`: a definition in a later record, a tempid that only `retract_entity` names
	/// (it still uses up an entity id), a tempid that only a ref names, an annotation of the transaction, an empty transaction,
	/// a non-ASCII string, and a transaction that fails (and so writes nothing)
	fn extras(conn: &mut Conn) -> Vec<TxReport>
	{
		let mut out = Vec::new();
		let mut tx = Tx::new();
		tx.define("note", ValueType::Str, Cardinality::Many);
		let (gone, target) = (tx.temp(), tx.temp());
		tx.retract_entity(gone).add(Entity::Tx, "note", "défini \"ici\"\n").add(Entity::Tx, "ref", target).with_instant(5000);
		out.push(conn.transact(&tx).unwrap());
		// A schema alteration and a compare-and-swap
		let mut tx = Tx::new();
		tx.alter_attr(Attr::new("note", ValueType::Str, Cardinality::One)).cas(Entity::Tx, "n", None, 3);
		out.push(conn.transact(&tx).unwrap());
		out.push(conn.transact(&Tx::new()).unwrap());
		let mut tx = Tx::new();
		tx.add(Entity::Tx, "n", 1).add(Entity::Tx, "n", 2);
		assert!(matches!(conn.transact(&tx), Err(Error::CardinalityConflict { .. })));
		out
	}

	/// A transaction to run on two connections that should be in the same state: it allocates an entity, upserts nothing,
	/// and takes the previous instant, so its report depends on the next entity id and the instant
	fn probe() -> Tx
	{
		let mut tx = Tx::new();
		let e = tx.temp();
		tx.add(e, "key", 1000).add(e, "tag", "probe").add(Entity::Tx, "n", 7);
		tx
	}

	fn same_report(x: &TxReport, y: &TxReport)
	{
		assert_eq!((x.tx, &x.tempids, &x.lookups, &x.tx_data), (y.tx, &y.tempids, &y.lookups, &y.tx_data));
	}

	fn file_len(path: &Path) -> usize
	{
		fs::metadata(path).unwrap().len() as usize
	}

	#[test]
	fn crc32_check_value()
	{
		assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
		assert_eq!(crc32(b""), 0);
	}

	// A durable connection's history, reopened, is the same as an in-memory connection's with the same transactions, at every basis
	#[test]
	fn reopening_replays_the_whole_history()
	{
		let scratch = Scratch::new("round_trip");
		let path = scratch.path("db.log");
		let mut mem = Conn::new();
		let mut mem_reports = random_workload(&mut mem, 23, 60);
		mem_reports.extend(extras(&mut mem));
		let n = mem.db().basis_t();
		{
			let mut conn = Conn::open(&path).unwrap();
			assert_eq!(conn.db().basis_t(), 0);
			let mut reports = random_workload(&mut conn, 23, 60);
			let len = file_len(&path);
			reports.extend(extras(&mut conn));
			assert!(file_len(&path) > len);
			for (x, y) in reports.iter().zip(&mem_reports)
			{
				same_report(x, y);
			}
			for t in 0..=n
			{
				assert_same_db(&conn.as_of(t).unwrap(), &mem.as_of(t).unwrap(), "durable connection");
			}
			// The failed transaction wrote nothing: the file ends with the last committed record
			let bytes = fs::read(&path).unwrap();
			let (recs, end) = records(&bytes).unwrap();
			assert_eq!((recs.len() as u64, end), (n, bytes.len()));
			assert_eq!(decode(recs.last().unwrap().1).unwrap(), reports.last().unwrap().record());
		}
		let mut conn = Conn::open(&path).unwrap();
		for t in 0..=n
		{
			assert_same_db(&conn.as_of(t).unwrap(), &mem.as_of(t).unwrap(), &format!("reopened, basis {t}"));
		}
		assert!(conn.as_of(n + 1).is_none());
		// The reopened connection continues exactly where the original would have: the same entity ids and instant
		let (x, y) = (conn.transact(&probe()).unwrap(), mem.transact(&probe()).unwrap());
		same_report(&x, &y);
		assert_eq!(conn.db().attr(x.tx_data[0].a).name.as_ref(), TX_INSTANT);
		drop(conn);
		let conn = Conn::open(&path).unwrap();
		assert_same_db(&conn.db(), &mem.db(), "reopened after appending");
		// The records round-trip through the encoding
		for r in &mem_reports
		{
			assert_eq!(decode(&encode(&r.record())).unwrap(), r.record());
		}
	}

	// A file cut anywhere in its last record (or its header) opens as the history without it, is truncated to the complete records,
	// and takes the transaction again
	#[test]
	fn a_torn_final_record_is_truncated()
	{
		let scratch = Scratch::new("torn");
		let path = scratch.path("db.log");
		let mut mem = Conn::new();
		random_workload(&mut mem, 5, 12);
		let last = mem.transact(&probe()).unwrap();
		let (start, end, bytes) = {
			let mut conn = Conn::open(&path).unwrap();
			random_workload(&mut conn, 5, 12);
			let start = file_len(&path);
			conn.transact(&probe()).unwrap();
			(start, file_len(&path), fs::read(&path).unwrap())
		};
		let n = mem.db().basis_t();
		let cuts: Vec<usize> = (0..=FILE_HEADER).chain(start..=end).collect();
		for cut in cuts
		{
			fs::write(&path, &bytes[..cut]).unwrap();
			let mut conn = Conn::open(&path).unwrap();
			let (basis, len) = match cut
			{
				c if c <= FILE_HEADER => (0, FILE_HEADER),
				c if c == end => (n, end),
				_ => (n - 1, start),
			};
			assert_eq!(file_len(&path), len, "cut at {cut}");
			assert_same_reads(&conn.db(), &mem.as_of(basis).unwrap(), &format!("cut at {cut}"));
			if basis == n - 1
			{
				same_report(&conn.transact(&probe()).unwrap(), &last);
				drop(conn);
				assert_eq!(fs::read(&path).unwrap(), bytes, "cut at {cut}");
				assert_same_db(&Conn::open(&path).unwrap().db(), &mem.db(), &format!("cut at {cut}, reopened"));
			}
		}
	}

	/// The log of a few transactions (the last ones retracting and asserting), with the offset of each record and the end of the file
	fn small_log(path: &Path) -> (Vec<u8>, Vec<usize>)
	{
		let mut conn = Conn::open(path).unwrap();
		let mut starts = vec![file_len(path)];
		random_workload(&mut conn, 9, 1);
		for i in 0..6
		{
			starts.push(file_len(path));
			let mut tx = Tx::new();
			tx.add(Entity::Id(2 + i), "n", 100 + i as i64).add(Entity::Id(3 + i), "tag", "x");
			conn.transact(&tx).unwrap();
		}
		starts.push(file_len(path));
		(fs::read(path).unwrap(), starts)
	}

	fn open_err(path: &Path, bytes: &[u8]) -> Error
	{
		fs::write(path, bytes).unwrap();
		let err = Conn::open(path).err().expect("a corrupt log");
		assert_eq!(fs::read(path).unwrap(), bytes, "the file is left as it is");
		err
	}

	fn corrupt_at(err: Error) -> u64
	{
		match err
		{
			Error::CorruptLog { offset, .. } => offset,
			e => panic!("unexpected error {e}"),
		}
	}

	// Damage anywhere but a torn tail is reported at the damaged record, and the file is left as it is
	#[test]
	fn corruption_is_reported_not_truncated()
	{
		let scratch = Scratch::new("corrupt");
		let path = scratch.path("db.log");
		let (bytes, starts) = small_log(&path);
		let (s, e) = (starts[3], starts[4]);
		// Every change of one byte of a middle record (CRC-32 detects every error of up to 32 consecutive bits)
		for i in s..e
		{
			for mask in [0x01, 0x80, 0xFF]
			{
				let mut b = bytes.clone();
				b[i] ^= mask;
				assert_eq!(corrupt_at(open_err(&path, &b)), s as u64, "byte {i} mask {mask}");
			}
		}
		// A length running past the end of the file is not mistaken for a torn record, as its checksum fails
		let mut b = bytes.clone();
		b[s..s + 4].copy_from_slice(&(1u32 << 30).to_le_bytes());
		assert_eq!(corrupt_at(open_err(&path, &b)), s as u64);
		// A length with a valid checksum that doesn't end where the payload does
		let len = (e - s - RECORD_HEADER + 1) as u32;
		b[s..s + 4].copy_from_slice(&len.to_le_bytes());
		b[s + 4..s + 8].copy_from_slice(&crc32(&len.to_le_bytes()).to_le_bytes());
		assert_eq!(corrupt_at(open_err(&path, &b)), s as u64);
		// A complete last record with a bad checksum is corruption, not a torn write
		let last = starts[starts.len() - 2];
		let mut b = bytes.clone();
		*b.last_mut().unwrap() ^= 1;
		assert_eq!(corrupt_at(open_err(&path, &b)), last as u64);
		// The file header
		let mut b = bytes.clone();
		b[0] ^= 1;
		assert_eq!(corrupt_at(open_err(&path, &b)), 0);
		let mut b = bytes.clone();
		b[8] = 2;
		assert!(matches!(open_err(&path, &b), Error::CorruptLog { offset: 8, msg } if msg.contains("version 2")));
		assert_eq!(corrupt_at(open_err(&path, b"satdbX")), 0);
		assert_eq!(corrupt_at(open_err(&path, b"hello, this is not a database log at all")), 0);
		// The undamaged file still opens
		fs::write(&path, &bytes).unwrap();
		assert_eq!(Conn::open(&path).unwrap().db().basis_t(), 7);
	}

	// Well-framed records that don't fit the database before them are corruption: each is refused by Db::replay or by the decoder
	#[test]
	fn records_that_dont_fit_are_refused()
	{
		let scratch = Scratch::new("misfit");
		let path = scratch.path("db.log");
		let (bytes, starts) = small_log(&path);
		let last = starts[starts.len() - 2];
		let rec = decode(&bytes[last + RECORD_HEADER..]).unwrap();
		assert!(!rec.retracted.is_empty() && rec.asserted.len() == 2);
		let first = decode(&bytes[starts[0] + RECORD_HEADER..starts[1]]).unwrap();
		let with = |payload: &[u8]| -> Error
		{
			let mut b = bytes[..last].to_vec();
			b.extend(frame(payload).unwrap());
			open_err(&path, &b)
		};
		let check = |f: &dyn Fn(&mut TxRecord), want: &str|
		{
			let mut r = rec.clone();
			f(&mut r);
			match with(&encode(&r))
			{
				Error::CorruptLog { offset, msg } => assert!(offset == last as u64 && msg.contains(want), "{msg} (wanted {want})"),
				e => panic!("unexpected error {e}"),
			}
		};
		// The record itself is fine
		let mut b = bytes[..last].to_vec();
		b.extend(frame(&encode(&rec)).unwrap());
		assert_eq!(b, bytes);
		check(&|r| r.basis_t -= 1, "basis");
		check(&|r| r.tx += 1, "next entity");
		check(&|r| r.next_eid = r.tx, "next entity");
		check(&|r| r.instant = -1, "instant");
		check(&|r| r.asserted.push(r.asserted[1].clone()), "increasing");
		check(&|r| r.asserted.push((r.tx, 99, Value::Int(0))), "unknown attribute");
		check(&|r| r.asserted[0].2 = Value::from("x"), "invalid datom");
		check(&|r| r.asserted[1].0 = r.next_eid, "invalid datom");
		check(&|r| r.retracted[0].2 = Value::Int(-5), "retraction of an absent");
		check(&|r| r.retracted.clear(), "cardinality or uniqueness");
		check(&|r| { r.asserted.push(first.asserted.last().unwrap().clone()); r.asserted.sort() }, "assertion of a present");
		check(&|r| r.asserted.push((r.tx, first.asserted[0].1, Value::Ref(0))), "invalid datom");
		check(&|r| r.attrs.push((0, Attr::new("again", ValueType::Int, Cardinality::One))), "id 0");
		check(&|r| r.attrs.push((99, Attr::new("again", ValueType::Int, Cardinality::One))), "id 99");
		// A string that isn't UTF-8, and trailing bytes
		let mut r = rec.clone();
		r.asserted[1].2 = Value::from("\u{7f}\u{7f}");
		let mut p = encode(&r);
		let at = p.windows(2).position(|w| w == [0x7f, 0x7f]).unwrap();
		p[at] = 0xFF;
		assert!(matches!(with(&p), Error::CorruptLog { msg, .. } if msg.contains("UTF-8")));
		let mut p = encode(&rec);
		p.push(0);
		assert!(matches!(with(&p), Error::CorruptLog { msg, .. } if msg.contains("after the record")));
		// A record replayed twice
		let mut b = bytes.clone();
		b.extend_from_slice(&bytes[last..]);
		assert!(matches!(open_err(&path, &b), Error::CorruptLog { offset, msg } if offset == bytes.len() as u64 && msg.contains("basis")));
	}

	// The decoder fails cleanly on any bytes: every prefix of a valid payload, random bytes and random changes of a valid payload
	#[test]
	fn decoding_is_total()
	{
		let mut mem = Conn::new();
		let reports = random_workload(&mut mem, 3, 30);
		let mut rng = Rng::new(8);
		for r in &reports
		{
			let p = encode(&r.record());
			for cut in 0..p.len()
			{
				assert!(decode(&p[..cut]).is_err());
			}
			for _ in 0..50
			{
				let mut q = p.clone();
				let i = (rng.next() % q.len() as u64) as usize;
				q[i] ^= 1 + (rng.next() % 255) as u8;
				// A change that still decodes encodes back to the same bytes
				if let Ok(rec) = decode(&q)
				{
					assert_eq!(encode(&rec), q);
				}
			}
		}
		for _ in 0..20000
		{
			let q: Vec<u8> = (0..rng.next() % 64).map(|_| rng.next() as u8).collect();
			let _ = decode(&q);
		}
	}

	// A transaction whose append fails isn't committed: it isn't visible, the file is cut back, and the next transaction takes its place;
	// if even the cut fails, the log refuses appends until it is reopened, which truncates the torn record
	#[test]
	fn a_failed_append_is_not_committed()
	{
		let scratch = Scratch::new("fault");
		let path = scratch.path("db.log");
		let mut mem = Conn::new();
		random_workload(&mut mem, 13, 10);
		let mut conn = Conn::open(&path).unwrap();
		random_workload(&mut conn, 13, 10);
		let (n, len) = (conn.db().basis_t(), file_len(&path));
		conn.log_mut().fault = 1;
		assert!(matches!(conn.transact(&probe()), Err(Error::Io(m)) if m.contains("injected")));
		assert_eq!((conn.db().basis_t(), file_len(&path)), (n, len));
		assert!(conn.as_of(n + 1).is_none());
		conn.log_mut().fault = 0;
		same_report(&conn.transact(&probe()).unwrap(), &mem.transact(&probe()).unwrap());
		let len = file_len(&path);
		conn.log_mut().fault = 2;
		let mut tx = Tx::new();
		tx.add(Entity::Id(3), "n", -1);
		assert!(matches!(conn.transact(&tx), Err(Error::Io(_))));
		assert!(file_len(&path) > len);
		conn.log_mut().fault = 0;
		assert!(matches!(conn.transact(&tx), Err(Error::Io(m)) if m.contains("reopen")));
		assert_eq!(conn.db().basis_t(), n + 1);
		drop(conn);
		let mut conn = Conn::open(&path).unwrap();
		assert_eq!(file_len(&path), len);
		assert_same_db(&conn.db(), &mem.db(), "reopened after a failed truncation");
		same_report(&conn.transact(&tx).unwrap(), &mem.transact(&tx).unwrap());
	}

	#[test]
	fn one_connection_per_log()
	{
		let scratch = Scratch::new("lock");
		let path = scratch.path("db.log");
		let mut conn = Conn::open(&path).unwrap();
		conn.transact(&Tx::new()).unwrap();
		assert!(matches!(Conn::open(&path), Err(Error::Io(m)) if m.contains("another connection")));
		drop(conn);
		assert_eq!(Conn::open(&path).unwrap().db().basis_t(), 1);
		assert!(matches!(Conn::open(scratch.path("")), Err(Error::Io(_))));
		assert!(matches!(Conn::open(scratch.path("missing/db.log")), Err(Error::Io(_))));
	}

	// A checkpoint opens to the same value as a full replay (and every past value reads the same), saves replaying the records it covers,
	// and is ignored, with the log replayed in full, whenever it doesn't fit: torn at any offset, damaged, of another log, or ahead of the log
	#[test]
	fn checkpoints_are_a_verified_cache()
	{
		let scratch = Scratch::new("checkpoints");
		let path = scratch.path("db.log");
		let ckpt = checkpoint_path(&path);
		assert_eq!(ckpt, scratch.path("db.log.ckpt"));
		let mut mem = Conn::new();
		let mut mem_reports = random_workload(&mut mem, 29, 40);
		mem_reports.extend(extras(&mut mem));
		let (n, len_at_ckpt) = {
			let mut conn = Conn::open(&path).unwrap();
			random_workload(&mut conn, 29, 40);
			extras(&mut conn);
			conn.checkpoint().unwrap();
			(conn.db().basis_t(), file_len(&path))
		};
		let full_db = {
			let bytes = fs::read(&path).unwrap();
			replay(&bytes, None).unwrap().0
		};
		fn check(conn: &Conn, mem: &Conn, from_ckpt: bool, what: &str)
		{
			assert_eq!(conn.log().from_checkpoint, from_ckpt, "{what}");
			assert_same_db(&conn.db(), &mem.db(), what);
			for t in (0..=mem.db().basis_t()).step_by(7)
			{
				assert_same_reads(&conn.as_of(t).unwrap(), &mem.as_of(t).unwrap(), &format!("{what}, as of {t}"));
			}
		}
		check(&Conn::open(&path).unwrap(), &mem, true, "from the checkpoint");
		assert_same_db(&full_db, &mem.db(), "full replay");

		// More transactions after the checkpoint: the tail is replayed on top of it
		{
			let mut conn = Conn::open(&path).unwrap();
			let (x, y) = (conn.transact(&probe()).unwrap(), mem.transact(&probe()).unwrap());
			same_report(&x, &y);
		}
		check(&Conn::open(&path).unwrap(), &mem, true, "checkpoint and tail");
		let good = fs::read(&ckpt).unwrap();
		// A torn checkpoint (cut anywhere) or a damaged one (a byte flipped anywhere, by a step) is ignored
		for cut in 0..good.len()
		{
			fs::write(&ckpt, &good[..cut]).unwrap();
			let conn = Conn::open(&path).unwrap();
			assert!(!conn.log().from_checkpoint, "cut at {cut}");
			assert_same_db(&conn.db(), &mem.db(), &format!("checkpoint cut at {cut}"));
		}
		for at in (0..good.len()).step_by(97)
		{
			let mut bad = good.clone();
			bad[at] ^= 0x20;
			fs::write(&ckpt, &bad).unwrap();
			check(&Conn::open(&path).unwrap(), &mem, false, &format!("byte {at} flipped"));
		}
		// The checkpoint of another log: a log with the same transactions but one, shorter than the covered prefix or not
		fs::write(&ckpt, &good).unwrap();
		let other = scratch.path("other.log");
		{
			let mut conn = Conn::open(&other).unwrap();
			random_workload(&mut conn, 30, 60);
		}
		fs::copy(&ckpt, checkpoint_path(&other)).unwrap();
		let conn = Conn::open(&other).unwrap();
		assert!(!conn.log().from_checkpoint);
		drop(conn);
		// ... and a log with the same records but for one value (so the same record boundaries and basis): only the fingerprint tells
		let twin = |name: &str, v: i64| {
			let path = scratch.path(name);
			let mut conn = Conn::open(&path).unwrap();
			let mut tx = Tx::new();
			let e = tx.temp();
			tx.define("x", ValueType::Int, Cardinality::One).add(e, "x", v);
			conn.transact(&tx).unwrap();
			path
		};
		let (a, b) = (twin("a.log", 1), twin("b.log", 2));
		Conn::open(&a).unwrap().checkpoint().unwrap();
		fs::copy(checkpoint_path(&a), checkpoint_path(&b)).unwrap();
		let conn = Conn::open(&b).unwrap();
		assert!(!conn.log().from_checkpoint);
		let x = conn.db().attr_id("x").unwrap();
		assert_eq!(conn.db().attr_datoms(x).map(|(_, v)| v.clone()).collect::<Vec<_>>(), vec![Value::Int(2)]);
		drop(conn);
		// A log cut back below the covered prefix (as if its later records were lost) makes the checkpoint useless too
		let bytes = fs::read(&path).unwrap();
		let (recs, _) = records(&bytes).unwrap();
		let cut = recs[recs.len() - 3].0;
		assert!(cut < len_at_ckpt);
		fs::write(&path, &bytes[..cut]).unwrap();
		let conn = Conn::open(&path).unwrap();
		assert!(!conn.log().from_checkpoint);
		assert_eq!(conn.db().basis_t(), n - 2);
		assert_same_db(&conn.db(), &replay(&bytes[..cut], None).unwrap().0, "shortened log");
		// An in-memory connection has no log to checkpoint
		assert!(matches!(Conn::new().checkpoint(), Err(Error::Io(_))));
	}

	/// A log of `txs` transactions of 20 assertions each (and some retractions), written without syncing (the test is about reading)
	fn big_log(path: &Path, txs: usize)
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("n", ValueType::Int, Cardinality::One).define("tag", ValueType::Str, Cardinality::Many).define("ref", ValueType::Ref, Cardinality::Many);
		let mut bytes = file_header().to_vec();
		bytes.extend(frame(&encode(&conn.transact(&tx).unwrap().record())).unwrap());
		let mut rng = Rng::new(3);
		let mut eids: Vec<Eid> = Vec::new();
		for _ in 1..txs
		{
			let mut tx = Tx::new();
			for _ in 0..20
			{
				let e = if eids.is_empty() || rng.next() % 4 == 0 { tx.temp() } else { Entity::Id(eids[(rng.next() % eids.len() as u64) as usize]) };
				match rng.next() % 3
				{
					0 => tx.add(e, "n", (rng.next() % 1000) as i64),
					1 => tx.add(e, "tag", ["a", "b", "c", "d"][(rng.next() % 4) as usize]),
					_ if !eids.is_empty() => tx.add(e, "ref", Entity::Id(eids[(rng.next() % eids.len() as u64) as usize])),
					_ => tx.add(e, "n", 0),
				};
			}
			if let Ok(r) = conn.transact(&tx)
			{
				eids.extend(r.tempids.iter().copied());
				bytes.extend(frame(&encode(&r.record())).unwrap());
			}
		}
		fs::write(path, bytes).unwrap();
	}

	// The time to open a durable log (release: cargo test --release open_timings -- --ignored --nocapture)
	#[test]
	#[ignore]
	fn open_timings()
	{
		let scratch = Scratch::new("open_timings");
		for txs in [1_000, 10_000, 50_000]
		{
			let path = scratch.path(&format!("{txs}.log"));
			big_log(&path, txs);
			let t = std::time::Instant::now();
			let mut conn = Conn::open(&path).unwrap();
			let replayed = t.elapsed();
			let t = std::time::Instant::now();
			conn.checkpoint().unwrap();
			let written = t.elapsed();
			let db = conn.db();
			drop(conn);
			let t = std::time::Instant::now();
			let conn = Conn::open(&path).unwrap();
			let from_ckpt = t.elapsed();
			assert!(conn.log().from_checkpoint);
			assert_same_db(&conn.db(), &db, "reopened from the checkpoint");
			let ckpt = fs::metadata(checkpoint_path(&path)).unwrap().len();
			println!("{txs} transactions, {} bytes: open by replay {replayed:?}; checkpoint ({ckpt} bytes) written in {written:?}; open from it {from_ckpt:?}", file_len(&path));
		}
	}
}
