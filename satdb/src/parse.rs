//! A text syntax for queries: a subset of Datomic's Datalog, written in EDN, read into the `Query` builder.
//!
//! ```text
//! [:find ?name (count ?f)
//!  :with ?x
//!  :in $ % ?city
//!  :where [?p :person/city ?city]
//!         [?p :person/name ?name]
//!         [?p :person/follows ?f]
//!         [(> ?age 30)]
//!         (not [?f :person/city ?city])
//!         (not-join [?p] [?p :person/banned true])
//!         (or [?f :person/age 40] (and [?f :person/vip true] [?f :person/age 41]))
//!         (or-join [?p] [?p :person/admin true] [?x :person/boss ?p])
//!         (follows* ?p ?f)]
//!
//! ; the rules (%)
//! [[(follows* ?a ?b) [?a :person/follows ?b]]
//!  [(follows* ?a ?b) [?a :person/follows ?c] (follows* ?c ?b)]]
//! ```
//! * The sections `:find`, `:with`, `:in` and `:where`, each at most once, in any order; `:find` is required.
//! * `:find` takes variables and aggregates `(count ?x)`, `(count-distinct ?x)`, `(sum ?x)`, `(min ?x)`, `(max ?x)`, `(avg ?x)`
//!   (see `Query::find`); `:with` takes variables.
//! * `:in` takes `$` (the database; one at most, and it may be left out), `%` (the rules, given as a text of their own to
//!   `Db::q_rules`) and bindings, each bound to the `Input` at its position among them: a scalar `?x` (`Input::Scalar`) and a tuple
//!   `[?a ?b]` (`Input::Tuple`) become constants of the query (so the planner sees them as bound); a collection `[?x ...]`
//!   (`Input::Coll`) and a relation `[[?a ?b]]` (`Input::Rel`) become base relations (`Query::relation`) that the query joins, like
//!   rule calls, so its solutions (and its aggregates) range over all of them. `_` in a binding ignores that position.
//! * Rules are written as in Datomic, `[[(name ?a ?b) clause ...] ...]`: a head of distinct variables, then the body; definitions
//!   with the same name are a union, and rules can call each other in any order. A call `(name arg ...)` is a clause, in the query
//!   or in a rule, whose arguments are terms or lookup refs (see the `rules` module). Required variables (`(name [?a] ?b)`) are not supported.
//!   The rules' variables are their own (an input named like one of them doesn't replace it), and their syntax errors say `in the rules`.
//! * Patterns `[e :attr v tx added]` of 2 to 5 elements (missing positions are ignored); the attribute is a keyword constant.
//!   `_` is a variable of its own at every occurrence, a *blank*, and so is each missing position. Blanks are never join variables:
//!   `(not [?p :person/friend _])` is `(not-join [?p] ...)`, and the branches of `or` need the same variables apart from their blanks.
//!   A lookup ref `[:attr value]` names an entity in the entity or value position.
//! * Comparisons `[(op a b)]` with `=`, `!=` (or `not=`), `<`, `<=`, `>` and `>=`.
//! * `(not clause ...)`, `(not-join [?v ...] clause ...)`, `(or branch ...)` and `(or-join [?v ...] branch ...)`, where a branch
//!   is a clause or `(and clause ...)`.
//! * Constants: strings (with the escapes `\"`, `\\`, `\n`, `\t` and `\r`), integers, floats (`1.5`, `-2e3`), booleans and instants
//!   (`#inst "2024-02-29T13:45:00Z"`, RFC 3339 with `Z` or an offset, or a date alone). There is no keyword value type,
//!   so a keyword can only name an attribute.
//! * Pull patterns (`Db::pull_str`, not in queries' find specs) are EDN too, the only place where maps are allowed.
//!
//! Commas are whitespace and `;` starts a comment. Syntax errors are `Error::Parse`, with the line and column; the errors of the clauses
//! themselves (an unknown attribute, a type mismatch, ...) are those of the builder.

use crate::db::{Db, Eid, Error, Value};
use crate::query::{Agg, Clause, Cmp, Find, Pull, Pulled, Query, Term};
use std::collections::{BTreeSet, HashMap};

/// An input of a text query, for the binding in its `:in` section at the same position (`$` and `%` take none)
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Input
{
	/// For `?x`
	Scalar(Value),
	/// For `[?x ...]`: the query's solutions for each value
	Coll(Vec<Value>),
	/// For `[?a ?b]`: a value for each variable
	Tuple(Vec<Value>),
	/// For `[[?a ?b]]`: the query's solutions for each tuple
	Rel(Vec<Vec<Value>>),
}

impl From<Value> for Input
{
	fn from(v: Value) -> Self
	{
		Input::Scalar(v)
	}
}

/// A parsed text query: the query (planned as its clauses were read), its find spec and its `with` terms
pub struct TextQuery
{
	pub query: Query,
	pub find: Vec<Find>,
	pub with: Vec<Term>,
}

impl TextQuery
{
	/// Optimize the plan and run the query (see `Query::find`)
	pub fn run(&mut self) -> Result<Vec<Vec<Value>>, Error>
	{
		self.query.find(&self.find, &self.with)
	}
}

impl Db
{
	/// Parse a text query (see the module documentation), with `inputs` bound to the bindings of its `:in` section after `$` and `%`, in order
	pub fn parse_query(&self, text: &str, inputs: &[Input]) -> Result<TextQuery, Error>
	{
		self.parse(text, None, inputs)
	}

	/// Parse a text query that takes the rule set `rules` (as `%` in its `:in` section; see the module documentation)
	pub fn parse_query_rules(&self, text: &str, rules: &str, inputs: &[Input]) -> Result<TextQuery, Error>
	{
		self.parse(text, Some(rules), inputs)
	}

	fn parse(&self, text: &str, rules: Option<&str>, inputs: &[Input]) -> Result<TextQuery, Error>
	{
		let node = Reader { text, pos: 0 }.read_all()?;
		Parser { text, q: self.query(), inputs: HashMap::new(), fresh: 0, rule_names: HashMap::new() }.query(node, rules, inputs)
	}

	/// Parse and run a text query
	pub fn q(&self, text: &str, inputs: &[Input]) -> Result<Vec<Vec<Value>>, Error>
	{
		self.parse_query(text, inputs)?.run()
	}

	/// Parse and run a text query that takes the rule set `rules`
	pub fn q_rules(&self, text: &str, rules: &str, inputs: &[Input]) -> Result<Vec<Vec<Value>>, Error>
	{
		self.parse_query_rules(text, rules, inputs)?.run()
	}

	/// Pull entity `e` by a pull pattern in Datomic's EDN syntax (see `Db::pull`):
	/// `[:person/name *  :person/_friend  [:person/aka :limit 2 :as "aliases"]  [:person/age :default 0]
	///   {:person/friend [:person/name]}  {[:person/boss :as "chain"] ...}  {:person/friend 2}]`.
	/// A keyword whose name starts with `_` is a reverse attribute; a map nests a pattern under each of its keys, or recurses
	/// (`...`: until an entity repeats, or a depth)
	pub fn pull_str(&self, pattern: &str, e: Eid) -> Result<Pulled, Error>
	{
		let node = Reader { text: pattern, pos: 0 }.read_all()?;
		self.pull(&pull_pattern(pattern, &node)?, e)
	}
}

/// A pull pattern of EDN (see `Db::pull_str`)
fn pull_pattern(text: &str, node: &Node) -> Result<Vec<Pull>, Error>
{
	let Form::Vector(items) = &node.form
	else
	{
		return Err(error_at(text, node.pos, "a pull pattern is a vector"));
	};
	let mut out = Vec::new();
	for item in items
	{
		match &item.form
		{
			Form::Symbol(s) if s == "*" => out.push(Pull::Wildcard),
			Form::Keyword(_) | Form::Vector(_) | Form::List(_) => out.push(pull_attr(text, item)?),
			Form::Map(pairs) =>
			{
				for pair in pairs.chunks(2)
				{
					let attr = pull_attr(text, &pair[0])?;
					out.push(match &pair[1].form
					{
						Form::Symbol(s) if s == "..." => attr.recurse(None),
						Form::Int(n) if *n >= 0 => attr.recurse(Some(*n as usize)),
						Form::Vector(_) => attr.nest(pull_pattern(text, &pair[1])?),
						_ => return Err(error_at(text, pair[1].pos, "a map of a pull pattern takes a pattern, ... or a depth for each attribute")),
					});
				}
			}
			_ => return Err(error_at(text, item.pos, "expected an attribute, *, an attribute with options or a map")),
		}
	}
	Ok(out)
}

/// An attribute of a pull pattern: `:ns/name` (`:ns/_name` for the reverse), or `[:ns/name option value ...]` with the options
/// `:as` (a string or a keyword), `:limit` (a count) and `:default` (a constant)
fn pull_attr(text: &str, node: &Node) -> Result<Pull, Error>
{
	let err = |n: &Node, msg: &str| error_at(text, n.pos, msg);
	let named = |n: &Node| match &n.form
	{
		Form::Keyword(k) => Ok(match k.rsplit_once('/')
		{
			Some((ns, local)) if local.len() > 1 && local.starts_with('_') => Pull::reverse(&format!("{ns}/{}", &local[1..])),
			None if k.len() > 1 && k.starts_with('_') => Pull::reverse(&k[1..]),
			_ => Pull::attr(k),
		}),
		_ => Err(err(n, "expected an attribute keyword")),
	};
	let (Form::Vector(items) | Form::List(items)) = &node.form else { return named(node) };
	let Some((first, opts)) = items.split_first() else { return Err(err(node, "an attribute with options needs the attribute")) };
	if opts.len() % 2 == 1
	{
		return Err(err(node, "every option of a pull attribute needs a value"));
	}
	let mut attr = named(first)?;
	for pair in opts.chunks(2)
	{
		let (k, v) = (&pair[0], &pair[1]);
		attr = match (&k.form, &v.form)
		{
			(Form::Keyword(o), Form::Str(s) | Form::Keyword(s)) if o == "as" => attr.key(s),
			(Form::Keyword(o), Form::Int(n)) if o == "limit" && *n >= 0 => attr.limit(*n as usize),
			(Form::Keyword(o), _) if o == "default" => attr.default(match &v.form
			{
				Form::Str(s) => Value::from(s.as_str()),
				Form::Int(i) => Value::Int(*i),
				Form::Float(f) => Value::Float(*f),
				Form::Bool(b) => Value::Bool(*b),
				Form::Instant(i) => Value::Instant(*i),
				_ => return Err(err(v, "a default is a constant")),
			}),
			(Form::Keyword(o), _) if ["as", "limit"].contains(&o.as_str()) => return Err(err(v, "invalid value of the option")),
			_ => return Err(err(k, "unknown option of a pull attribute (the options are :as, :limit and :default)")),
		};
	}
	Ok(attr)
}

/// The error at byte offset `pos` of `text`
fn error_at(text: &str, pos: usize, msg: impl Into<String>) -> Error
{
	let before = &text[..pos];
	let line = before.matches('\n').count() + 1;
	let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
	Error::Parse { line, col, msg: msg.into() }
}

#[derive(Debug)]
enum Form
{
	Vector(Vec<Node>),
	List(Vec<Node>),
	/// The keys and values, alternating (only pull patterns take maps)
	Map(Vec<Node>),
	Keyword(String),
	Symbol(String),
	Str(String),
	Int(i64),
	Float(f64),
	/// `#inst "..."`, in milliseconds since the Unix epoch
	Instant(i64),
	Bool(bool),
}

/// The instant of an RFC 3339 timestamp (`2024-02-29T13:45:00Z`, `...:00.25+02:00`; also a date alone, at midnight UTC),
/// in milliseconds since the Unix epoch (fractions beyond milliseconds are truncated); None if it isn't one
pub(crate) fn parse_instant(s: &str) -> Option<i64>
{
	let num = |s: &str| -> Option<i64> { (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten() };
	let (date, time) = s.split_once(['T', 't']).unwrap_or((s, "00:00:00Z"));
	let mut d = date.split('-');
	let (y, m, day) = (d.next().filter(|x| x.len() == 4).and_then(num)?, d.next().filter(|x| x.len() == 2).and_then(num)?, d.next().filter(|x| x.len() == 2).and_then(num)?);
	if d.next().is_some() || !(1..=12).contains(&m)
	{
		return None;
	}
	let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
	let days_in_month = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m as usize - 1];
	if !(1..=days_in_month).contains(&day)
	{
		return None;
	}
	// The offset: Z, or +hh:mm / -hh:mm
	let (clock, offset) = if let Some(c) = time.strip_suffix(['Z', 'z'])
	{
		(c, 0)
	}
	else
	{
		let at = time.rfind(['+', '-'])?;
		let (h, mi) = time[at + 1..].split_once(':')?;
		let (h, mi) = (Some(h).filter(|x| x.len() == 2).and_then(num)?, Some(mi).filter(|x| x.len() == 2).and_then(num)?);
		if h > 23 || mi > 59
		{
			return None;
		}
		let sign = if &time[at..at + 1] == "-" { -1 } else { 1 };
		(&time[..at], sign * (h * 60 + mi))
	};
	let (hms, frac) = clock.split_once('.').map_or((clock, None), |(a, b)| (a, Some(b)));
	let mut c = hms.split(':');
	let (hh, mm, ss) = (c.next().filter(|x| x.len() == 2).and_then(num)?, c.next().filter(|x| x.len() == 2).and_then(num)?, c.next().filter(|x| x.len() == 2).and_then(num)?);
	if c.next().is_some() || hh > 23 || mm > 59 || ss > 60
	{
		return None;
	}
	let millis = match frac
	{
		None => 0,
		Some(f) =>
		{
			num(f)?;
			format!("{:0<3}", &f[..f.len().min(3)]).parse::<i64>().ok()?
		}
	};
	// Days since the epoch of the civil date (the proleptic Gregorian calendar)
	let (yy, mm_) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
	let era = yy.div_euclid(400);
	let yoe = yy - era * 400;
	let doy = (153 * mm_ + 2) / 5 + day - 1;
	let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
	let days = era * 146_097 + doe - 719_468;
	Some(((days * 24 + hh) * 60 + mm - offset) * 60_000 + ss * 1000 + millis)
}

/// A form and its byte offset in the text
#[derive(Debug)]
struct Node
{
	form: Form,
	pos: usize,
}

/// The EDN subset reader
struct Reader<'a>
{
	text: &'a str,
	pos: usize,
}

fn is_delimiter(c: char) -> bool
{
	c.is_whitespace() || "[](){}\",;".contains(c)
}

impl<'a> Reader<'a>
{
	fn peek(&self) -> Option<char>
	{
		self.text[self.pos..].chars().next()
	}

	fn skip_space(&mut self)
	{
		while let Some(c) = self.peek()
		{
			if c == ';'
			{
				self.pos = self.text[self.pos..].find('\n').map_or(self.text.len(), |i| self.pos + i);
			}
			else if c.is_whitespace() || c == ','
			{
				self.pos += c.len_utf8();
			}
			else
			{
				break;
			}
		}
	}

	// Exactly one form
	fn read_all(mut self) -> Result<Node, Error>
	{
		let node = self.read()?;
		self.skip_space();
		if self.pos < self.text.len()
		{
			return Err(error_at(self.text, self.pos, "unexpected text after the query"));
		}
		Ok(node)
	}

	fn read(&mut self) -> Result<Node, Error>
	{
		self.skip_space();
		let pos = self.pos;
		let Some(c) = self.peek()
		else
		{
			return Err(error_at(self.text, pos, "unexpected end of the query"));
		};
		let form = match c
		{
			'[' | '(' | '{' =>
			{
				self.pos += 1;
				let close = match c
				{
					'[' => ']',
					'(' => ')',
					_ => '}',
				};
				let mut items = Vec::new();
				loop
				{
					self.skip_space();
					match self.peek()
					{
						None => return Err(error_at(self.text, pos, format!("{c} is never closed"))),
						Some(d) if d == close =>
						{
							self.pos += 1;
							break;
						}
						Some(d @ (']' | ')' | '}')) => return Err(error_at(self.text, self.pos, format!("expected {close} (to close the {c} at {}), found {d}", self.place(pos)))),
						Some(_) => items.push(self.read()?),
					}
				}
				match c
				{
					'[' => Form::Vector(items),
					'(' => Form::List(items),
					_ if items.len() % 2 == 1 => return Err(error_at(self.text, pos, "a map needs a value for every key")),
					_ => Form::Map(items),
				}
			}
			']' | ')' | '}' => return Err(error_at(self.text, pos, format!("unexpected {c}"))),
			'"' => Form::Str(self.string()?),
			'#' if self.text[pos..].starts_with("#inst") && self.text[pos + 5..].starts_with(|c: char| c.is_whitespace() || c == '"') =>
			{
				self.pos += 5;
				self.skip_space();
				if self.peek() != Some('"')
				{
					return Err(error_at(self.text, self.pos, "#inst takes a string (an RFC 3339 timestamp)"));
				}
				let at = self.pos;
				let s = self.string()?;
				Form::Instant(parse_instant(&s).ok_or_else(|| error_at(self.text, at, format!("{s:?} is not an RFC 3339 timestamp (e.g. \"2024-02-29T13:45:00Z\")")))?)
			}
			'#' | '\'' | '`' | '~' | '^' | '@' | '\\' => return Err(error_at(self.text, pos, format!("{c} is not supported (no sets, tags other than #inst, quotes or characters)"))),
			_ =>
			{
				let token = self.token();
				if let Some(name) = token.strip_prefix(':')
				{
					if name.is_empty()
					{
						return Err(error_at(self.text, pos, "a keyword needs a name"));
					}
					Form::Keyword(name.to_string())
				}
				else if token.strip_prefix(['+', '-']).unwrap_or(token).starts_with(|d: char| d.is_ascii_digit())
				{
					let digits = token.strip_prefix(['+', '-']).unwrap_or(token);
					if digits.chars().all(|d| d.is_ascii_digit())
					{
						Form::Int(token.parse().map_err(|_| error_at(self.text, pos, format!("the integer {token} is out of range")))?)
					}
					// A float: digits with a fraction and/or an exponent (Rust's syntax for f64 is a superset of EDN's, so check the characters first)
					else if digits.chars().all(|d| d.is_ascii_digit() || ".eE+-".contains(d)) && digits.contains(['.', 'e', 'E'])
					{
						Form::Float(token.parse().map_err(|_| error_at(self.text, pos, format!("invalid number {token}")))?)
					}
					else
					{
						return Err(error_at(self.text, pos, format!("invalid number {token}")));
					}
				}
				else
				{
					match token
					{
						"true" => Form::Bool(true),
						"false" => Form::Bool(false),
						"nil" => return Err(error_at(self.text, pos, "nil is not supported")),
						_ => Form::Symbol(token.to_string()),
					}
				}
			}
		};
		Ok(Node { form, pos })
	}

	fn place(&self, pos: usize) -> String
	{
		match error_at(self.text, pos, "")
		{
			Error::Parse { line, col, .. } => format!("{line}:{col}"),
			_ => unreachable!(),
		}
	}

	fn token(&mut self) -> &'a str
	{
		let start = self.pos;
		while let Some(c) = self.peek()
		{
			if is_delimiter(c)
			{
				break;
			}
			self.pos += c.len_utf8();
		}
		&self.text[start..self.pos]
	}

	fn string(&mut self) -> Result<String, Error>
	{
		let start = self.pos;
		self.pos += 1;
		let mut out = String::new();
		loop
		{
			let Some(c) = self.peek()
			else
			{
				return Err(error_at(self.text, start, "the string is never closed"));
			};
			self.pos += c.len_utf8();
			match c
			{
				'"' => return Ok(out),
				'\\' =>
				{
					let at = self.pos - 1;
					let e = self.peek().ok_or_else(|| error_at(self.text, start, "the string is never closed"))?;
					self.pos += e.len_utf8();
					out.push(match e
					{
						'"' => '"',
						'\\' => '\\',
						'n' => '\n',
						't' => '\t',
						'r' => '\r',
						_ => return Err(error_at(self.text, at, format!("unknown escape \\{e}"))),
					});
				}
				c => out.push(c),
			}
		}
	}
}

/// The reader of a query's forms into the builder
/// A binding of the `:in` section (None: `_`, a position that binds nothing)
enum Binding
{
	Scalar(String),
	Coll(Option<String>),
	Tuple(Vec<Option<String>>),
	Rel(Vec<Option<String>>),
}

impl Binding
{
	fn vars(&self) -> Box<dyn Iterator<Item = Option<&String>> + '_>
	{
		match self
		{
			Binding::Scalar(x) => Box::new(std::iter::once(Some(x))),
			Binding::Coll(x) => Box::new(std::iter::once(x.as_ref())),
			Binding::Tuple(xs) | Binding::Rel(xs) => Box::new(xs.iter().map(|x| x.as_ref())),
		}
	}
}

struct Parser<'a>
{
	text: &'a str,
	q: Query,
	/// The scalar inputs, by variable name
	inputs: HashMap<String, Value>,
	/// The number of blanks so far (each `_` and each missing position of a pattern is a variable of its own, see `blank`)
	fresh: usize,
	/// The rules' names and arities
	rule_names: HashMap<String, usize>,
}

impl<'a> Parser<'a>
{
	fn err(&self, node: &Node, msg: impl Into<String>) -> Error
	{
		error_at(self.text, node.pos, msg)
	}

	fn query(mut self, node: Node, rules: Option<&'a str>, inputs: &[Input]) -> Result<TextQuery, Error>
	{
		let Form::Vector(items) = node.form
		else
		{
			return Err(self.err(&node, "a query is a vector [:find ... :where ...]"));
		};
		// The sections, by keyword
		let mut sections: Vec<(String, Node, Vec<Node>)> = Vec::new();
		for item in items
		{
			match &item.form
			{
				Form::Keyword(k) =>
				{
					if !["find", "with", "in", "where"].contains(&k.as_str())
					{
						return Err(self.err(&item, format!("unknown section :{k} (the sections are :find, :with, :in and :where)")));
					}
					if sections.iter().any(|s| s.0 == *k)
					{
						return Err(self.err(&item, format!("a second :{k} section")));
					}
					sections.push((k.clone(), item, Vec::new()));
				}
				_ => match sections.last_mut()
				{
					Some(s) => s.2.push(item),
					None => return Err(self.err(&item, "a query starts with :find")),
				},
			}
		}
		let mut take = |name: &str| sections.iter().position(|s| s.0 == name).map(|i| sections.remove(i));
		let (find, with_section, input, clauses) = (take("find"), take("with"), take("in"), take("where"));
		let Some((_, find_kw, find)) = find
		else
		{
			return Err(error_at(self.text, node.pos, "the query has no :find section"));
		};
		if find.is_empty()
		{
			return Err(self.err(&find_kw, ":find needs at least one variable or aggregate"));
		}

		// The inputs first: scalars and tuples become constants of the clauses, collections and relations base relations that the query joins
		let mut bindings: Vec<Binding> = Vec::new();
		let mut takes_rules = false;
		if let Some((_, _, names)) = input
		{
			let mut sources = 0;
			let mut seen: Vec<String> = Vec::new();
			for n in &names
			{
				let binding = match &n.form
				{
					Form::Symbol(s) if s == "$" =>
					{
						sources += 1;
						if sources > 1
						{
							return Err(self.err(n, "only one database input ($) is supported"));
						}
						continue;
					}
					Form::Symbol(s) if s == "%" =>
					{
						if takes_rules
						{
							return Err(self.err(n, "only one rule set (%) is supported"));
						}
						if rules.is_none()
						{
							return Err(self.err(n, "the query takes rules (%), but none were given (see Db::q_rules)"));
						}
						takes_rules = true;
						continue;
					}
					Form::Symbol(s) if s.starts_with('?') => Binding::Scalar(s.clone()),
					Form::Vector(items) => match &items[..]
					{
						[x, Node { form: Form::Symbol(dots), .. }] if dots == "..." => Binding::Coll(self.binding_var(x)?),
						[Node { form: Form::Vector(vars), .. }] if !vars.is_empty() => Binding::Rel(vars.iter().map(|v| self.binding_var(v)).collect::<Result<_, _>>()?),
						[] => return Err(self.err(n, "an empty binding")),
						_ if items.iter().any(|v| matches!(&v.form, Form::Symbol(s) if s == "...")) => return Err(self.err(n, "a collection binding is [?x ...]")),
						vars => Binding::Tuple(vars.iter().map(|v| self.binding_var(v)).collect::<Result<_, _>>()?),
					},
					_ => return Err(self.err(n, "an input is $, %, a variable ?x, a collection [?x ...], a tuple [?a ?b] or a relation [[?a ?b]]")),
				};
				for v in binding.vars().flatten()
				{
					if seen.contains(v)
					{
						return Err(self.err(n, format!("the input {v} is named twice")));
					}
					seen.push(v.clone());
				}
				bindings.push(binding);
			}
		}
		if bindings.len() != inputs.len()
		{
			return Err(Error::InvalidQuery(match bindings.len()
			{
				0 => format!("the query takes no inputs, got {}", inputs.len()),
				n => format!("the query takes {n} inputs, got {}", inputs.len()),
			}));
		}
		// The rules before the inputs are installed: a rule's variables are its own, even when an input has the same name
		match rules
		{
			Some(r) if takes_rules => self.rules(r)?,
			Some(_) => return Err(Error::InvalidQuery("rules were given, but the query's :in has no %".into())),
			None => {}
		}
		let mut calls = Vec::new();
		for (k, (b, input)) in bindings.into_iter().zip(inputs).enumerate()
		{
			let mismatch = |want: &str| Error::InvalidQuery(format!("input {} is for {want}", k + 1));
			let tuples = match (b, input)
			{
				(Binding::Scalar(x), Input::Scalar(v)) =>
				{
					self.inputs.insert(x, v.clone());
					continue;
				}
				(Binding::Tuple(xs), Input::Tuple(vs)) if xs.len() == vs.len() =>
				{
					self.inputs.extend(xs.into_iter().zip(vs.iter().cloned()).filter_map(|(x, v)| Some((x?, v))));
					continue;
				}
				(Binding::Coll(x), Input::Coll(vs)) => (vec![x], vs.iter().map(|v| vec![v.clone()]).collect()),
				(Binding::Rel(xs), Input::Rel(ts)) => (xs, ts.clone()),
				(Binding::Scalar(_), _) => return Err(mismatch("a scalar binding (Input::Scalar)")),
				(Binding::Tuple(xs), _) => return Err(mismatch(&format!("a tuple binding of {} (Input::Tuple)", xs.len()))),
				(Binding::Coll(_), _) => return Err(mismatch("a collection binding (Input::Coll)")),
				(Binding::Rel(_), _) => return Err(mismatch("a relation binding (Input::Rel)")),
			};
			// A name that no query can write
			let name = format!(":in {}", k + 1);
			self.q.relation(&name, tuples.0.len(), tuples.1)?;
			calls.push((name, tuples.0));
		}
		for (name, xs) in calls
		{
			let mut args = Vec::with_capacity(xs.len());
			for x in xs
			{
				args.push(match x
				{
					Some(x) => self.q.var(&x),
					None => self.blank(),
				});
			}
			self.q.call(&name, &args)?;
		}

		for c in clauses.map(|s| s.2).unwrap_or_default()
		{
			let c = self.clause(&c)?;
			self.q.add_clause(c)?;
		}

		let mut spec = Vec::new();
		for f in &find
		{
			spec.push(match &f.form
			{
				Form::Symbol(s) if s == "." || s == "..." => return Err(self.err(f, "scalar and collection find specs are not supported")),
				Form::Symbol(s) if s.starts_with('?') => Find::Term(self.term(f)?),
				Form::List(items) =>
				{
					let agg = match items.first().map(|h| &h.form)
					{
						Some(Form::Symbol(name)) => Agg::from_name(name).ok_or_else(|| self.err(&items[0], format!("unknown aggregate {name} (the aggregates are count, count-distinct, sum, min, max and avg)")))?,
						_ => return Err(self.err(f, "an aggregate is (name ?var)")),
					};
					match &items[1..]
					{
						[x] if matches!(&x.form, Form::Symbol(s) if s.starts_with('?')) => Find::Agg(agg, self.term(x)?),
						_ => return Err(self.err(f, format!("({} ?var) takes one variable", agg.name()))),
					}
				}
				_ => return Err(self.err(f, ":find takes variables and aggregates")),
			});
		}
		let mut with = Vec::new();
		for w in with_section.map(|s| s.2).unwrap_or_default()
		{
			match &w.form
			{
				Form::Symbol(s) if s.starts_with('?') => with.push(self.term(&w)?),
				_ => return Err(self.err(&w, ":with takes variables")),
			}
		}
		Ok(TextQuery { query: self.q, find: spec, with })
	}

	// A variable of an input binding, or `_`
	fn binding_var(&self, node: &Node) -> Result<Option<String>, Error>
	{
		match &node.form
		{
			Form::Symbol(s) if s == "_" => Ok(None),
			Form::Symbol(s) if s.starts_with('?') => Ok(Some(s.clone())),
			_ => Err(self.err(node, "a binding takes variables ?x (or _)")),
		}
	}

	// A variable (an input's value, for an input) or a constant
	fn term(&mut self, node: &Node) -> Result<Term, Error>
	{
		match &node.form
		{
			Form::Symbol(s) if s == "_" => Ok(self.blank()),
			Form::Symbol(s) if s.starts_with('?') => Ok(match self.inputs.get(s)
			{
				Some(v) => Term::Val(v.clone()),
				None => self.q.var(s),
			}),
			Form::Symbol(s) => Err(self.err(node, format!("expected a variable or a constant, found the symbol {s}"))),
			Form::Str(s) => Ok(Term::Val(s.as_str().into())),
			Form::Int(i) => Ok(Term::Val((*i).into())),
			Form::Float(f) => Ok(Term::Val((*f).into())),
			Form::Instant(i) => Ok(Term::Val(Value::Instant(*i))),
			Form::Bool(b) => Ok(Term::Val((*b).into())),
			Form::Keyword(k) => Err(self.err(node, format!("the keyword :{k} is not a value (keywords only name attributes)"))),
			Form::Vector(_) | Form::List(_) | Form::Map(_) => Err(self.err(node, "expected a variable or a constant")),
		}
	}

	// A term in the entity or value position of a pattern, where a lookup ref [:attr value] names an entity
	fn entity_term(&mut self, node: &Node) -> Result<Term, Error>
	{
		match &node.form
		{
			Form::Vector(items) => match &items[..]
			{
				[Node { form: Form::Keyword(attr), .. }, v] => match self.term(v)?
				{
					Term::Val(v) => self.q.lookup(attr, v),
					Term::Var(_) => Err(self.err(v, "the value of a lookup ref must be a constant")),
				},
				_ => Err(self.err(node, "a lookup ref is [:attr value]")),
			},
			_ => self.term(node),
		}
	}

	fn clause(&mut self, node: &Node) -> Result<Clause, Error>
	{
		match &node.form
		{
			Form::Vector(items) if matches!(items.first(), Some(Node { form: Form::List(_), .. })) =>
			{
				if let Some(extra) = items.get(1)
				{
					return Err(self.err(extra, "function bindings ([(f ...) ?out]) are not supported"));
				}
				self.pred(&items[0])
			}
			Form::Vector(items) => self.pattern(node, items),
			Form::List(items) =>
			{
				let head = match items.first()
				{
					Some(Node { form: Form::Symbol(s), .. }) => s.as_str(),
					_ => return Err(self.err(node, "a clause list starts with not, not-join, or or or-join")),
				};
				let rest = &items[1..];
				match head
				{
					"not" => self.not(node, None, rest),
					"not-join" =>
					{
						let (join, body) = self.join_list(node, rest)?;
						self.not(node, Some(join), body)
					}
					"or" => self.or(node, None, rest),
					"or-join" =>
					{
						let (join, branches) = self.join_list(node, rest)?;
						self.or(node, Some(join), branches)
					}
					"and" => Err(self.err(node, "and is only a branch of or")),
					_ if self.op(head).is_some() => Err(self.err(node, format!("a predicate is written in a vector: [({head} ...)]"))),
					_ if self.rule_names.contains_key(head) => self.call(node, head, rest),
					_ => Err(self.err(&items[0], format!("unknown clause {head} (the clause lists are not, not-join, or, or-join and calls of the rules given with %)"))),
				}
			}
			_ => Err(self.err(node, "expected a clause: a pattern [...], a predicate [(...)], or a not or or list")),
		}
	}

	fn pattern(&mut self, node: &Node, items: &[Node]) -> Result<Clause, Error>
	{
		if !(2..=5).contains(&items.len())
		{
			return Err(self.err(node, "a pattern has 2 to 5 elements: [e :attr v tx added]"));
		}
		let attr = match &items[1].form
		{
			Form::Keyword(k) => k.clone(),
			Form::Symbol(s) if s.starts_with('?') || s == "_" => return Err(self.err(&items[1], "attribute variables are not supported")),
			_ => return Err(self.err(&items[1], "the attribute of a pattern is a keyword")),
		};
		let e = self.entity_term(&items[0])?;
		let mut rest = Vec::new();
		for (i, n) in items.iter().enumerate().skip(2)
		{
			rest.push(if i == 2 { self.entity_term(n)? } else { self.term(n)? });
		}
		let mut rest = rest.into_iter();
		let mut next = |p: &mut Self| rest.next().unwrap_or_else(|| p.blank());
		let v = next(self);
		if items.len() <= 3
		{
			return self.q.pattern_clause(e, &attr, v);
		}
		let (tx, added) = (next(self), next(self));
		self.q.pattern_tx_clause(e, &attr, v, tx, added)
	}

	// A new blank: a variable named _n, which no variable of the text can be named (they start with ?)
	fn blank(&mut self) -> Term
	{
		self.fresh += 1;
		self.q.var(&format!("_{}", self.fresh))
	}

	// The variables of `clauses` that an implicit not or or joins on: their free variables, without the blanks
	fn implicit_join(&self, clauses: &[Clause]) -> Vec<Term>
	{
		let mut vs = BTreeSet::new();
		clauses.iter().for_each(|c| c.free_vars(&mut vs));
		vs.into_iter().filter(|&x| !self.q.var_name(x).starts_with('_')).map(Term::Var).collect()
	}

	fn op(&self, name: &str) -> Option<Cmp>
	{
		Some(match name
		{
			"=" => Cmp::Eq,
			"!=" | "not=" => Cmp::Ne,
			"<" => Cmp::Lt,
			"<=" => Cmp::Le,
			">" => Cmp::Gt,
			">=" => Cmp::Ge,
			_ => return None,
		})
	}

	fn pred(&mut self, node: &Node) -> Result<Clause, Error>
	{
		let Form::List(items) = &node.form
		else
		{
			unreachable!("a predicate is a list")
		};
		let op = match items.first()
		{
			Some(h @ Node { form: Form::Symbol(s), .. }) => self.op(s).ok_or_else(|| self.err(h, format!("unknown predicate {s} (the predicates are = != not= < <= > >=)")))?,
			_ => return Err(self.err(node, "a predicate is (op a b)")),
		};
		match &items[1..]
		{
			[l, r] =>
			{
				let (l, r) = (self.term(l)?, self.term(r)?);
				Ok(Clause::pred(l, op, r))
			}
			_ => Err(self.err(node, "a comparison takes two arguments")),
		}
	}

	// The [?v ...] list of not-join or or-join and the forms after it
	fn join_list<'n>(&mut self, node: &Node, rest: &'n [Node]) -> Result<(Vec<Term>, &'n [Node]), Error>
	{
		let Some(Node { form: Form::Vector(vars), .. }) = rest.first()
		else
		{
			return Err(self.err(node, "a join list [?v ...] follows not-join and or-join"));
		};
		let mut join = Vec::new();
		for v in vars
		{
			match &v.form
			{
				Form::Vector(_) => return Err(self.err(v, "required variables ([[?v ...] ...]) are not supported")),
				Form::Symbol(s) if s.starts_with('?') =>
				{
					// An input is a constant, which joins by itself
					if let Term::Var(x) = self.term(v)?
					{
						join.push(Term::Var(x));
					}
				}
				_ => return Err(self.err(v, "a join list holds variables")),
			}
		}
		Ok((join, &rest[1..]))
	}

	// A call (name arg ...) of a rule; the arguments are terms or lookup refs
	fn call(&mut self, node: &Node, name: &str, args: &[Node]) -> Result<Clause, Error>
	{
		let arity = self.rule_names[name];
		if args.len() != arity
		{
			return Err(self.err(node, format!("the rule {name} takes {arity} arguments, got {}", args.len())));
		}
		let args = args.iter().map(|a| self.entity_term(a)).collect::<Result<Vec<_>, _>>()?;
		self.q.call_clause(name, &args)
	}

	// The rule set: [[(name ?a ...) clause ...] ...], read with its own positions (the messages say so)
	fn rules(&mut self, rules: &'a str) -> Result<(), Error>
	{
		let text = std::mem::replace(&mut self.text, rules);
		let r = self.read_rules();
		self.text = text;
		r.map_err(|e| match e
		{
			Error::Parse { line, col, msg } => Error::Parse { line, col, msg: format!("in the rules: {msg}") },
			e => e,
		})
	}

	fn read_rules(&mut self) -> Result<(), Error>
	{
		let node = Reader { text: self.text, pos: 0 }.read_all()?;
		let Form::Vector(rules) = &node.form
		else
		{
			return Err(self.err(&node, "the rules are a vector [[(name ?a ...) clause ...] ...]"));
		};
		// The heads first, so that the rules can call each other in any order
		let mut defs: Vec<(String, Vec<String>, &[Node])> = Vec::new();
		for r in rules
		{
			let (items, head) = match &r.form
			{
				Form::Vector(items) => match items.first()
				{
					Some(Node { form: Form::List(head), .. }) => (items, head),
					_ => return Err(self.err(r, "a rule starts with its head (name ?a ...)")),
				},
				_ => return Err(self.err(r, "a rule is a vector [(name ?a ...) clause ...]")),
			};
			let name = match head.first()
			{
				Some(n @ Node { form: Form::Symbol(s), .. }) =>
				{
					let reserved = ["not", "not-join", "or", "or-join", "and", "_", "$", "%"].contains(&s.as_str()) || self.op(s).is_some() || s.starts_with('?');
					if reserved
					{
						return Err(self.err(n, format!("{s} can't name a rule")));
					}
					s.clone()
				}
				Some(n) => return Err(self.err(n, "a rule's name is a symbol")),
				None => return Err(self.err(&items[0], "a rule's head (name ?a ...) needs a name")),
			};
			let mut vars: Vec<String> = Vec::new();
			for a in &head[1..]
			{
				match &a.form
				{
					Form::Symbol(s) if s.starts_with('?') =>
					{
						if vars.contains(s)
						{
							return Err(self.err(a, format!("the head of rule {name} has {s} twice")));
						}
						vars.push(s.clone());
					}
					Form::Vector(_) => return Err(self.err(a, "required variables ([?a] in a rule's head) are not supported")),
					_ => return Err(self.err(a, "a rule's head has variables only")),
				}
			}
			match self.rule_names.get(&name)
			{
				Some(&k) if k != vars.len() => return Err(self.err(&items[0], format!("the rule {name} takes {k} arguments in another definition, here {}", vars.len()))),
				_ => self.rule_names.insert(name.clone(), vars.len()),
			};
			if items.len() == 1
			{
				return Err(self.err(r, format!("the rule {name} has no clauses")));
			}
			defs.push((name, vars, &items[1..]));
		}
		for (name, vars, body) in defs
		{
			let head: Vec<Term> = vars.iter().map(|v| self.q.var(v)).collect();
			let body = self.body(body)?;
			self.q.rule(&name, &head, body)?;
		}
		Ok(())
	}

	fn body(&mut self, forms: &[Node]) -> Result<Vec<Clause>, Error>
	{
		forms.iter().map(|f| self.clause(f)).collect()
	}

	fn not(&mut self, node: &Node, join: Option<Vec<Term>>, body: &[Node]) -> Result<Clause, Error>
	{
		if body.is_empty()
		{
			return Err(self.err(node, "not needs at least one clause"));
		}
		let body = self.body(body)?;
		let join = join.unwrap_or_else(|| self.implicit_join(&body));
		self.q.not(Some(&join), body)
	}

	fn or(&mut self, node: &Node, join: Option<Vec<Term>>, forms: &[Node]) -> Result<Clause, Error>
	{
		if forms.is_empty()
		{
			return Err(self.err(node, "or needs at least one branch"));
		}
		let mut branches = Vec::new();
		for f in forms
		{
			branches.push(match &f.form
			{
				Form::List(items) if matches!(items.first(), Some(Node { form: Form::Symbol(s), .. }) if s == "and") =>
				{
					if items.len() == 1
					{
						return Err(self.err(f, "and needs at least one clause"));
					}
					self.body(&items[1..])?
				}
				_ => vec![self.clause(f)?],
			});
		}
		let join = match join
		{
			Some(j) => j,
			None =>
			{
				let sets: Vec<Vec<Term>> = branches.iter().map(|b| self.implicit_join(b)).collect();
				if let Some(i) = sets.iter().position(|s| *s != sets[0])
				{
					let name = |t: &Term| if let Term::Var(x) = t { self.q.var_name(*x) } else { unreachable!("join lists hold variables") };
					let names = |s: &[Term]| s.iter().map(name).collect::<Vec<_>>().join(" ");
					return Err(Error::InvalidQuery(format!("the branches of or use different variables: [{}] and [{}] (or-join names the shared ones)", names(&sets[0]), names(&sets[i]))));
				}
				sets.into_iter().next().unwrap_or_default()
			}
		};
		self.q.or(Some(&join), branches)
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::db::tests::{random_conn, replay};
	use crate::db::{AttrId, Cardinality, Conn, Eid, Tx, ValueType};
	use std::collections::BTreeSet;

	fn parse_error(db: &Db, text: &str) -> (usize, usize, String)
	{
		match db.parse_query(text, &[]).err()
		{
			Some(Error::Parse { line, col, msg }) => (line, col, msg),
			e => panic!("expected a parse error for {text}, got {e:?}"),
		}
	}

	fn refs(es: impl Iterator<Item = Eid>) -> Vec<Vec<Value>>
	{
		es.collect::<BTreeSet<_>>().into_iter().map(|e| vec![Value::Ref(e)]).collect()
	}

	// A text query reads into exactly the clauses the builder makes, and gives the same answers
	#[test]
	fn text_matches_the_builder()
	{
		let (mut conn, reports) = random_conn(71, 60);
		let db = conn.db();
		let text = r#"
			; the entities with an n of at least 1, no tag "a", and a ref or a best whose n is 2
			[:find ?e ?v
			 :in $ ?min
			 :where [?e :n ?v]
			        [(>= ?v ?min)]
			        (not [?e :tag "a"])
			        (or-join [?e] [?e :ref ?f] (and [?e :best ?g] [?g :n 2]))]"#;
		let mut tq = db.parse_query(text, &[Value::Int(1).into()]).unwrap();
		let mut q = db.query();
		let (e, v, f, g) = (q.var("?e"), q.var("?v"), q.var("?f"), q.var("?g"));
		q.pattern(e.clone(), "n", v.clone()).unwrap();
		q.pred(v.clone(), Cmp::Ge, 1).unwrap();
		let tag = q.pattern_clause(e.clone(), "tag", "a").unwrap();
		let not = q.not(None, vec![tag]).unwrap();
		q.add_clause(not).unwrap();
		let branches = vec![vec![q.pattern_clause(e.clone(), "ref", f.clone()).unwrap()], vec![q.pattern_clause(e.clone(), "best", g.clone()).unwrap(), q.pattern_clause(g.clone(), "n", 2).unwrap()]];
		let or = q.or(Some(&[e.clone()]), branches).unwrap();
		q.add_clause(or).unwrap();
		assert_eq!(tq.query.clauses(), q.clauses());
		assert_eq!(tq.find, vec![Find::Term(e.clone()), Find::Term(v.clone())]);
		let got = tq.run().unwrap();
		assert!(!got.is_empty());
		assert_eq!(got, q.run(&[e.clone(), v.clone()]).unwrap());

		// Aggregates and with read into the find spec, which runs as `Query::find`
		let text = "[:find ?e (count ?v) (count-distinct ?v) (sum ?v) (min ?v) (max ?v) (avg ?v) :with ?f :where [?e :ref ?f] [?f :n ?v]]";
		let mut tq = db.parse_query(text, &[]).unwrap();
		let (e, f, v) = (Term::Var(0), Term::Var(1), Term::Var(2));
		let aggs = [Agg::Count, Agg::CountDistinct, Agg::Sum, Agg::Min, Agg::Max, Agg::Avg];
		let spec: Vec<Find> = [Find::Term(e.clone())].into_iter().chain(aggs.map(|a| Find::Agg(a, v.clone()))).collect();
		assert_eq!(tq.find, spec);
		assert_eq!(tq.with, vec![f.clone()]);
		let mut q = db.query();
		let (e2, f2, v2) = (q.var("?e"), q.var("?f"), q.var("?v"));
		q.pattern(e2, "ref", f2.clone()).unwrap();
		q.pattern(f2, "n", v2).unwrap();
		assert_eq!(tq.run().unwrap(), q.find(&spec, &[f]).unwrap());

		// Each _ is a variable of its own: the entities with both a tag and a ref (not a tag equal to a ref, which can't be)
		let log: Vec<_> = reports.iter().flat_map(|r| &r.tx_data).collect();
		let facts: BTreeSet<(Eid, AttrId, Value)> = replay(log.iter().copied()).into_keys().collect();
		let id = |n: &str| db.attr_id(n).unwrap();
		let has = |e: Eid, a: &str| facts.iter().any(|f| f.0 == e && f.1 == id(a));
		let want = refs(facts.iter().map(|f| f.0).filter(|&e| has(e, "tag") && has(e, "ref")));
		assert!(!want.is_empty());
		assert_eq!(db.q("[:find ?e :where [?e :tag _] [?e :ref _]]", &[]).unwrap(), want);
		// A two-element pattern, commas, and the database input in any position
		let target = facts.iter().find(|f| f.1 == id("ref") && has(f.0, "tag")).unwrap();
		let got = db.q("[:find ?e, :in ?a $ :where [?e :tag], [?e :ref ?a]]", &[target.2.clone().into()]).unwrap();
		assert_eq!(got, refs(facts.iter().filter(|f| f.1 == id("ref") && f.2 == target.2 && has(f.0, "tag")).map(|f| f.0)));

		// A lookup ref, in the entity position and as a value
		let (holder, k) = facts.iter().find(|f| f.1 == id("key")).map(|f| (f.0, f.2.clone())).unwrap();
		let Value::Int(k) = k else { unreachable!() };
		assert_eq!(db.q(&format!("[:find ?e ?v :where [[:key {k}] :key ?v] [?e :key ?v]]"), &[]).unwrap(), vec![vec![Value::Ref(holder), Value::Int(k)]]);
		let got = db.q(&format!("[:find ?e :where [?e :ref [:key {k}]]]"), &[]).unwrap();
		assert_eq!(got, refs(facts.iter().filter(|f| f.1 == id("ref") && f.2 == Value::Ref(holder)).map(|f| f.0)));
		assert!(matches!(db.q("[:find ?e :where [?e :ref [:key 999]]]", &[]), Err(Error::LookupNotFound { .. })));

		// Five elements over the history: the retractions of tags, with their transactions
		let want: BTreeSet<Vec<Value>> = log.iter().filter(|d| d.a == id("tag") && !d.added).map(|d| vec![Value::Ref(d.e), d.v.clone(), Value::Ref(d.tx)]).collect();
		assert!(!want.is_empty());
		assert_eq!(db.history().q("[:find ?e ?v ?tx :where [?e :tag ?v ?tx false]]", &[]).unwrap(), want.into_iter().collect::<Vec<_>>());
		assert_eq!(db.history().q("[:find ?e ?v :where [?e :tag ?v _ false]]", &[]).unwrap().len(), log.iter().filter(|d| d.a == id("tag") && !d.added).map(|d| (d.e, &d.v)).collect::<BTreeSet<_>>().len());

		// Strings with escapes
		let mut tx = crate::db::Tx::new();
		let t = tx.temp();
		tx.add(t, "tag", "say \"hi\"\n\t\\");
		let t = conn.transact(&tx).unwrap().eid(t);
		assert_eq!(conn.db().q(r#"[:find ?e :where [?e :tag "say \"hi\"\n\t\\"]]"#, &[]).unwrap(), vec![vec![Value::Ref(t)]]);
	}

	// Blanks (`_`, and the missing positions of a short pattern) are never join variables of an implicit not or or,
	// as in Datomic: (not [?e :ref _]) keeps the entities without a ref, and (or [?e :tag _] [?e :ref _]) joins on ?e only
	#[test]
	fn blanks_dont_join()
	{
		let (conn, reports) = random_conn(71, 60);
		let db = conn.db();
		let log: Vec<_> = reports.iter().flat_map(|r| &r.tx_data).collect();
		let facts: BTreeSet<(Eid, AttrId, Value)> = replay(log.iter().copied()).into_keys().collect();
		let id = |n: &str| db.attr_id(n).unwrap();
		let has = |e: Eid, a: &str| facts.iter().any(|f| f.0 == e && f.1 == id(a));
		let with_n: Vec<Eid> = facts.iter().filter(|f| f.1 == id("n")).map(|f| f.0).collect();
		let want = refs(with_n.iter().copied().filter(|&e| !has(e, "ref")));
		assert!(!want.is_empty() && want.len() < with_n.iter().collect::<BTreeSet<_>>().len());
		for text in ["[:find ?e :where [?e :n _] (not [?e :ref _])]", "[:find ?e :where [?e :n] (not [?e :ref])]", "[:find ?e :where [?e :n _] (not-join [?e] [?e :ref _])]"]
		{
			assert_eq!(db.q(text, &[]).unwrap(), want, "{text}");
		}
		let want = refs(with_n.iter().copied().filter(|&e| has(e, "tag") || has(e, "ref")));
		for text in
			["[:find ?e :where [?e :n _] (or [?e :tag _] [?e :ref _])]", "[:find ?e :where [?e :n] (or [?e :tag] (and [?e :ref _] [?e :n _]))]", "[:find ?e :where (or [?e :tag _] [?e :ref _]) [?e :n _]]"]
		{
			assert_eq!(db.q(text, &[]).unwrap(), want, "{text}");
		}
		// A body of blanks only joins on nothing: the rows survive only if no datom matches
		let any_ref = facts.iter().any(|f| f.1 == id("ref"));
		assert!(any_ref);
		assert_eq!(db.q("[:find ?e :where [?e :n _] (not [_ :ref _])]", &[]).unwrap(), Vec::<Vec<Value>>::new());
		assert_eq!(db.q("[:find ?e :where [?e :n _] (or [_ :ref _] [_ :tag \"no such tag\"])]", &[]).unwrap(), refs(with_n.iter().copied()));
		// Named variables still have to agree between the branches of or
		assert!(matches!(db.q("[:find ?e :where [?e :n _] (or [?e :tag _] [?f :ref _])]", &[]), Err(Error::InvalidQuery(_))));
	}

	#[test]
	fn errors_have_positions()
	{
		let (conn, _) = random_conn(3, 5);
		let db = conn.db();
		let at = |text: &str| {
			let (line, col, _) = parse_error(&db, text);
			(line, col)
		};
		// The reader
		assert_eq!(at("[:find ?e :where [?e :n ?v]"), (1, 1));
		assert_eq!(at("[:find ?e :where [?e :n ?v)]"), (1, 27));
		assert_eq!(at("[:find ?e] ]"), (1, 12));
		assert_eq!(at("[:find ?e :where\n  [?e :n \"x]]"), (2, 10));
		assert_eq!(at("[:find ?e :where [?e :tag \"\\q\"]]"), (1, 28));
		assert_eq!(at("[:find ?e :where [?e :n 12x]]"), (1, 25));
		assert_eq!(at("[:find ?e :where [?e :n 99999999999999999999]]"), (1, 25));
		assert_eq!(at("[:find ?e :where {:a 1}]"), (1, 18));
		assert_eq!(at("[:find ?e :where [?e :n nil]]"), (1, 25));
		assert_eq!(at("[:find ?e :where [?e : 1]]"), (1, 22));
		assert_eq!(at("  "), (1, 3));
		// Columns count characters
		assert_eq!(at("[:find ?é :where [?é :n 1] ]]"), (1, 29));
		// The query
		assert_eq!(at("(:find ?e)"), (1, 1));
		assert_eq!(at("[?e :find ?e]"), (1, 2));
		assert_eq!(at("[:find ?e :limit 3]"), (1, 11));
		assert_eq!(at("[:find ?e :find ?e]"), (1, 11));
		assert_eq!(at("[:where [?e :n 1]]"), (1, 1));
		assert_eq!(at("[:find :where [?e :n 1]]"), (1, 2));
		assert_eq!(at("[:find ?e :where\n [?e :n :foo]]"), (2, 9));
		assert_eq!(at("[:find ?e :where [?e ?a 1]]"), (1, 22));
		assert_eq!(at("[:find ?e :where [?e \"n\" 1]]"), (1, 22));
		assert_eq!(at("[:find ?e :where [?e]]"), (1, 18));
		assert_eq!(at("[:find ?e :where [?e :n 1 _ true 9]]"), (1, 18));
		assert_eq!(at("[:find ?e :where [?e :n x]]"), (1, 25));
		assert_eq!(at("[:find ?e :where [(> ?e 1) ?x]]"), (1, 28));
		assert_eq!(at("[:find ?e :where [(like ?e 1)] [?e :n 1]]"), (1, 20));
		assert_eq!(at("[:find ?e :where [(> ?e)] [?e :n 1]]"), (1, 19));
		assert_eq!(at("[:find ?e :where (> ?e 1) [?e :n 1]]"), (1, 18));
		assert_eq!(at("[:find ?e :where (nope [?e :n 1])]"), (1, 19));
		assert_eq!(at("[:find ?e :where (and [?e :n 1])]"), (1, 18));
		assert_eq!(at("[:find ?e :where [?e :n 1] (not)]"), (1, 28));
		assert_eq!(at("[:find ?e :where [?e :n 1] (or-join [[?e]] [?e :tag \"a\"])]"), (1, 38));
		assert_eq!(at("[:find ?e :where [?e :n 1] (not-join ?e [?e :tag \"a\"])]"), (1, 28));
		assert_eq!(at("[:find ?e :where [?e :n 1] (or (and))]"), (1, 32));
		assert_eq!(at("[:find (median ?e) :where [?e :n 1]]"), (1, 9));
		assert_eq!(at("[:find (count ?e ?e) :where [?e :n 1]]"), (1, 8));
		assert_eq!(at("[:find ?e . :where [?e :n 1]]"), (1, 11));
		assert_eq!(at("[:find 3 :where [?e :n 1]]"), (1, 8));
		assert_eq!(at("[:find ?e :with 3 :where [?e :n 1]]"), (1, 17));
		assert_eq!(at("[:find ?e :in $ $ :where [?e :n 1]]"), (1, 17));
		assert_eq!(at("[:find ?e :in $ [?x ... ?y] :where [?e :n ?x]]"), (1, 17));
		assert_eq!(at("[:find ?e :in % :where [?e :n 1]]"), (1, 15));
		assert_eq!(at("[:find ?e :in ?x ?x :where [?e :n 1]]"), (1, 18));
		assert_eq!(at("[:find ?e :where [?e :n ?v] [[:key ?v] :n 1]]"), (1, 36));
		assert_eq!(at("[:find ?e :where [?e :n ?v] [[:key] :n 1]]"), (1, 30));
		assert_eq!(parse_error(&db, "[:find ?e :where [?e :n 1] (or)]").2, "or needs at least one branch");

		// The inputs, and the errors of the builder, which have no position
		assert_eq!(db.parse_query("[:find ?e :in $ ?x :where [?e :n ?x]]", &[]).err(), Some(Error::InvalidQuery("the query takes 1 inputs, got 0".into())));
		assert!(matches!(db.parse_query("[:find ?e :where [?e :n ?x]]", &[Value::Int(1).into()]), Err(Error::InvalidQuery(_))));
		assert_eq!(db.parse_query("[:find ?e :where [?e :nope 1]]", &[]).err(), Some(Error::UnknownAttr("nope".into())));
		assert!(matches!(db.parse_query("[:find ?e :where [?e :n \"x\"]]", &[]), Err(Error::TypeMismatch { .. })));
		assert!(matches!(db.parse_query("[:find ?e :where [?e :n 1] (or [?e :n 2] [?f :n 3])]", &[]), Err(Error::InvalidQuery(_))));
		assert_eq!(db.q("[:find ?x :where [?e :n 1]]", &[]).err(), Some(Error::Unbound("?x".into())));
		assert_eq!(db.q("[:find ?e :where (not [?e :n 1])]", &[]).err(), Some(Error::Unbound("?e".into())));
		// An input in a join list is a constant, which drops out of the list
		let tq = db.parse_query("[:find ?e :in ?x :where [?e :n ?x] (not-join [?x] [?e :tag \"a\"])]", &[Value::Int(1).into()]).unwrap();
		assert!(matches!(&tq.query.clauses()[1], Clause::Not { join, .. } if join.is_empty()));
	}

	const RULES: &str = r#"
		; reachability over ref, and walks of odd and even length, which call each other
		[[(reach ?x ?y) [?x :ref ?y]]
		 [(reach ?x ?y) [?x :ref ?z] (reach ?z ?y)]
		 [(odd ?x ?y) [?x :ref ?y]]
		 [(even ?x ?y) [?x :ref ?z] (odd ?z ?y)]
		 [(odd ?x ?y) [?x :ref ?z] (even ?z ?y)]]"#;

	// Rules read into exactly the definitions and calls the builder makes, and give the same answers
	#[test]
	fn text_rules_match_the_builder()
	{
		// A ring of refs through the entities with an n, of even length (so the parity of a walk is that of the distance)
		let (mut conn, _) = random_conn(71, 60);
		let db = conn.db();
		let ring: Vec<Eid> = db.attr_datoms(db.attr_id("n").unwrap()).map(|(e, _)| e).collect();
		assert!(ring.len() % 2 == 0 && ring.len() >= 4);
		let mut tx = crate::db::Tx::new();
		for (i, &e) in ring.iter().enumerate()
		{
			tx.add(crate::db::Entity::Id(e), "ref", crate::db::Entity::Id(ring[(i + 1) % ring.len()]));
		}
		conn.transact(&tx).unwrap();
		let db = conn.db();
		// ?x is an input of the query and a variable of the rules, which are their own
		let text = "[:find ?a ?b :in $ % ?x :where (reach ?a ?b) [?b :n ?x] (not (even ?a ?b))]";
		let mut tq = db.parse_query_rules(text, RULES, &[Value::Int(2).into()]).unwrap();
		let mut q = db.query();
		let (x, y, z) = (q.var("?x"), q.var("?y"), q.var("?z"));
		let edge = |q: &Query, a: &Term, b: &Term| q.pattern_clause(a.clone(), "ref", b.clone()).unwrap();
		let defs = [("reach", None), ("reach", Some("reach")), ("odd", None), ("even", Some("odd")), ("odd", Some("even"))];
		for (name, callee) in defs
		{
			let body = match callee
			{
				None => vec![edge(&q, &x, &y)],
				Some(c) => vec![edge(&q, &x, &z), q.call_clause(c, &[z.clone(), y.clone()]).unwrap()],
			};
			q.rule(name, &[x.clone(), y.clone()], body).unwrap();
		}
		let (a, b) = (q.var("?a"), q.var("?b"));
		q.call("reach", &[a.clone(), b.clone()]).unwrap();
		q.pattern(b.clone(), "n", 2).unwrap();
		let even = q.call_clause("even", &[a.clone(), b.clone()]).unwrap();
		let not = q.not(None, vec![even]).unwrap();
		q.add_clause(not).unwrap();
		assert_eq!(tq.query.clauses(), q.clauses());
		let got = tq.run().unwrap();
		assert!(!got.is_empty());
		assert_eq!(got, q.run(&[a.clone(), b.clone()]).unwrap());
		assert_eq!(tq.query.rule_stats(), q.rule_stats());
		assert_eq!(q.rule_stats().strata, 2);
		// A call with a lookup ref, and an aggregate over a rule
		let rows = db.q_rules("[:find ?x ?b :in $ % :where (reach ?x ?b)]", RULES, &[]).unwrap();
		let holders: Vec<(Eid, Value)> = db.attr_datoms(db.attr_id("key").unwrap()).map(|(e, v)| (e, v.clone())).collect();
		let (holder, Value::Int(k)) = holders.iter().find(|(e, _)| rows.iter().any(|r| r[0] == Value::Ref(*e))).cloned().unwrap() else { unreachable!() };
		let want: Vec<Vec<Value>> = rows.iter().filter(|r| r[0] == Value::Ref(holder)).map(|r| vec![r[1].clone()]).collect();
		assert_eq!(db.q_rules(&format!("[:find ?b :in $ % :where (reach [:key {k}] ?b)]"), RULES, &[]).unwrap(), want);
		assert_eq!(db.q_rules("[:find (count ?b) :with ?a :in $ % :where (reach ?a ?b)]", RULES, &[]).unwrap(), vec![vec![Value::Int(rows.len() as i64)]]);
	}

	#[test]
	fn rule_errors_have_positions()
	{
		let (conn, _) = random_conn(3, 5);
		let db = conn.db();
		// In the query
		let query_error = |text: &str| match db.parse_query_rules(text, RULES, &[]).err()
		{
			Some(Error::Parse { line, col, .. }) => (line, col),
			e => panic!("expected a parse error for {text}, got {e:?}"),
		};
		assert_eq!(query_error("[:find ?a :in $ % :where (nope ?a)]"), (1, 27));
		assert_eq!(query_error("[:find ?a :in $ % :where\n (reach ?a)]"), (2, 2));
		assert_eq!(query_error("[:find ?a :in $ % % :where (reach ?a ?a)]"), (1, 19));
		assert_eq!(query_error("[:find ?a :in $ % :where (reach ?a [:key])]"), (1, 36));
		// % without rules, and rules without %
		let (line, col, msg) = parse_error(&db, "[:find ?a :in $ % :where (reach ?a ?a)]");
		assert_eq!((line, col), (1, 17));
		assert!(msg.contains("Db::q_rules"));
		assert!(matches!(db.parse_query_rules("[:find ?a :where [?a :n 1]]", RULES, &[]), Err(Error::InvalidQuery(_))));
		// Without % the rules' names are unknown clauses
		assert_eq!(parse_error(&db, "[:find ?a :where (reach ?a ?a)]").1, 19);

		// In the rules, with their own positions
		let rule_error = |rules: &str| match db.parse_query_rules("[:find ?a :in $ % :where [?a :n 1]]", rules, &[]).err()
		{
			Some(Error::Parse { line, col, msg }) =>
			{
				assert!(msg.starts_with("in the rules: "), "{msg}");
				(line, col)
			}
			e => panic!("expected a parse error for {rules}, got {e:?}"),
		};
		assert_eq!(rule_error("(foo)"), (1, 1));
		assert_eq!(rule_error("[[(r ?x) [?x :n 1]]"), (1, 1));
		assert_eq!(rule_error("[(r ?x) [?x :n 1]]"), (1, 2));
		assert_eq!(rule_error("[[[?x :n 1]]]"), (1, 2));
		assert_eq!(rule_error("[[() [?x :n 1]]]"), (1, 3));
		assert_eq!(rule_error("[[(1 ?x) [?x :n 1]]]"), (1, 4));
		assert_eq!(rule_error("[[(not ?x) [?x :n 1]]]"), (1, 4));
		assert_eq!(rule_error("[[(< ?x) [?x :n 1]]]"), (1, 4));
		assert_eq!(rule_error("[[(r [?x]) [?x :n 1]]]"), (1, 6));
		assert_eq!(rule_error("[[(r ?x ?x) [?x :n 1]]]"), (1, 9));
		assert_eq!(rule_error("[[(r 1) [?x :n 1]]]"), (1, 6));
		assert_eq!(rule_error("[[(r ?x)]]"), (1, 2));
		assert_eq!(rule_error("[[(r ?x) [?x :n 1]] [(r ?x ?y) [?x :ref ?y]]]"), (1, 22));
		assert_eq!(rule_error("[[(r ?x) [?x :n 1]]\n [(s ?x) (r ?x ?x)]]"), (2, 10));
		assert_eq!(rule_error("[[(r ?x) [?x :n 1]]\n [(s ?x) [?x :n :one]]]"), (2, 17));

		// The builder's errors, without a position
		let run = |rules: &str| db.q_rules("[:find ?a :in $ % :where (r ?a)]", rules, &[]).err();
		assert_eq!(run("[[(r ?x) [?x :nope 1]]]"), Some(Error::UnknownAttr("nope".into())));
		assert_eq!(run("[[(r ?x) [?x :n ?y] [(> ?z 1)]]]"), Some(Error::Unbound("?z".into())));
		assert_eq!(run("[[(r ?x) [?y :n 1]]]"), Some(Error::Unbound("?x".into())));
		assert!(matches!(run("[[(r ?x) [?x :n _] (not (r ?x))]]"), Some(Error::InvalidQuery(m)) if m.contains("stratification")));
		// A rule's _ is a variable of its own, like the query's
		assert!(run("[[(r ?x) [?x :n _] [_ :ref ?x]]]").is_none());
	}

	// RFC 3339 timestamps against a naive count of days, and the forms that aren't timestamps
	#[test]
	fn instants()
	{
		fn naive(y: i64, m: i64, d: i64) -> i64
		{
			let leap = |y: i64| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
			let mut days = 0;
			for yy in 1970.min(y)..1970.max(y)
			{
				days += if leap(yy) { 366 } else { 365 };
			}
			if y < 1970
			{
				days = -days;
			}
			let months = [31, if leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
			days + months[..m as usize - 1].iter().sum::<i64>() + d - 1
		}
		for (y, m, d) in [(1970, 1, 1), (1969, 12, 31), (2000, 2, 29), (1900, 3, 1), (2024, 12, 31), (1600, 2, 29), (2100, 2, 28), (1, 1, 1)]
		{
			let s = format!("{y:04}-{m:02}-{d:02}");
			assert_eq!(parse_instant(&s), Some(naive(y, m, d) * 86_400_000), "{s}");
			assert_eq!(parse_instant(&format!("{s}T13:45:07.25Z")), Some(naive(y, m, d) * 86_400_000 + ((13 * 60 + 45) * 60 + 7) * 1000 + 250), "{s}");
		}
		assert_eq!(parse_instant("1970-01-01T02:00:00+02:00"), Some(0));
		assert_eq!(parse_instant("1969-12-31T19:30:00-04:30"), Some(0));
		assert_eq!(parse_instant("1970-01-01T00:00:00.123456789Z"), Some(123));
		for bad in ["", "2024-02-30", "2023-02-29", "2024-13-01", "2024-1-01", "24-01-01", "2024-01-01T25:00:00Z", "2024-01-01T10:00:00", "2024-01-01T10:00Z", "2024-01-01T10:00:00+2:00", "2024-01-01T10:00:00.Z", "x"]
		{
			assert_eq!(parse_instant(bad), None, "{bad}");
		}
	}

	// Collection, tuple and relation inputs, against scalar runs and the log
	#[test]
	fn collection_tuple_and_relation_inputs()
	{
		let (conn, reports) = random_conn(19, 40);
		let db = conn.db();
		let current = replay(reports.iter().flat_map(|r| &r.tx_data));
		let n = db.attr_id("n").unwrap();
		let ks: Vec<Value> = (0..6).map(Value::Int).collect();
		// A collection is the union of the scalar runs
		let coll = db.q("[:find ?e ?k :in $ [?k ...] :where [?e :n ?k]]", &[Input::Coll(ks.clone())]).unwrap();
		let mut union = BTreeSet::new();
		for k in &ks
		{
			for row in db.q("[:find ?e ?k :in $ ?k :where [?e :n ?k]]", &[k.clone().into()]).unwrap()
			{
				union.insert(row);
			}
		}
		assert_eq!(coll, union.into_iter().collect::<Vec<_>>());
		let want: Vec<Vec<Value>> = current.keys().filter(|(_, a, v)| *a == n && ks.contains(v)).map(|(e, _, v)| vec![Value::Ref(*e), v.clone()]).collect::<BTreeSet<_>>().into_iter().collect();
		assert!(!want.is_empty());
		assert_eq!(coll, want);
		// Aggregates run over the whole input, not per value
		assert_eq!(db.q("[:find (count ?e) :in $ [?k ...] :where [?e :n ?k]]", &[Input::Coll(ks.clone())]).unwrap(), vec![vec![Value::Int(want.len() as i64)]]);
		// A relation: the tuples of (entity, value) given, filtered by the data; a blank ignores a position
		let m = want.len().min(3);
		let rel: Vec<Vec<Value>> = want.iter().take(m).cloned().chain([vec![Value::Ref(9999), Value::Int(0)]]).collect();
		assert_eq!(db.q("[:find ?e ?k :in [[?e ?k]] $ :where [?e :n ?k]]", &[Input::Rel(rel.clone())]).unwrap(), want[..m].to_vec());
		assert_eq!(db.q("[:find ?e :in $ [[?e _]] :where [?e :n _]]", &[Input::Rel(rel.clone())]).unwrap(), want[..m].iter().map(|r| vec![r[0].clone()]).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>());
		// A tuple binds constants; with a blank, one of them
		let (e0, k0) = (want[0][0].clone(), want[0][1].clone());
		assert_eq!(db.q("[:find ?e :in $ [?e ?k] :where [?e :n ?k]]", &[Input::Tuple(vec![e0.clone(), k0.clone()])]).unwrap(), vec![vec![e0.clone()]]);
		assert_eq!(db.q("[:find ?k :in $ [?e _] :where [?e :n ?k]]", &[Input::Tuple(vec![e0.clone(), Value::Int(-1)])]).unwrap(), vec![vec![k0]]);
		// Several inputs, and an empty collection
		assert_eq!(db.q("[:find ?e :in $ [?k ...] ?e :where [?e :n ?k]]", &[Input::Coll(ks.clone()), e0.clone().into()]).unwrap(), vec![vec![e0]]);
		assert!(db.q("[:find ?e :in $ [?k ...] :where [?e :n ?k]]", &[Input::Coll(vec![])]).unwrap().is_empty());
		// Mismatches and malformed bindings
		assert!(matches!(db.q("[:find ?e :in $ [?k ...] :where [?e :n ?k]]", &[Value::Int(1).into()]), Err(Error::InvalidQuery(_))));
		assert!(matches!(db.q("[:find ?e :in $ [?e ?k] :where [?e :n ?k]]", &[Input::Tuple(vec![Value::Int(1)])]), Err(Error::InvalidQuery(_))));
		assert!(matches!(db.q("[:find ?e :in $ [[?e ?k]] :where [?e :n ?k]]", &[Input::Rel(vec![vec![Value::Int(1)]])]), Err(Error::InvalidQuery(_))));
		for bad in ["[:find ?e :in $ [] :where [?e :n _]]", "[:find ?e :in $ [[]] :where [?e :n _]]", "[:find ?e :in $ [?e 1] :where [?e :n _]]", "[:find ?e :in $ [?e ?e] :where [?e :n _]]", "[:find ?e :in $ [... ?e] :where [?e :n _]]"]
		{
			assert!(matches!(db.q(bad, &[Input::Tuple(vec![Value::Int(1), Value::Int(1)])]), Err(Error::Parse { .. })), "{bad}");
		}
	}

	// Pull patterns in EDN: the same as the builder's, and the options and recursion they express
	#[test]
	fn pull_patterns_in_text()
	{
		use crate::db::Entity;
		use crate::query::PullValue;
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("person/name", ValueType::Str, Cardinality::One)
			.define("person/aka", ValueType::Str, Cardinality::Many)
			.define("person/age", ValueType::Int, Cardinality::One)
			.define("person/friend", ValueType::Ref, Cardinality::Many)
			.define("person/boss", ValueType::Ref, Cardinality::One);
		// A chain of bosses a -> b -> c -> d, and a friendship cycle a <-> b
		let ps: Vec<Entity> = (0..4).map(|_| tx.temp()).collect();
		for (i, p) in ps.iter().enumerate()
		{
			tx.add(*p, "person/name", ["a", "b", "c", "d"][i]);
			if i + 1 < ps.len()
			{
				tx.add(*p, "person/boss", ps[i + 1]);
			}
		}
		tx.add(ps[0], "person/aka", "x").add(ps[0], "person/aka", "y").add(ps[0], "person/aka", "z").add(ps[0], "person/age", 30);
		tx.add(ps[0], "person/friend", ps[1]).add(ps[1], "person/friend", ps[0]);
		let r = conn.transact(&tx).unwrap();
		let db = conn.db();
		let e: Vec<Eid> = ps.iter().map(|p| r.eid(*p)).collect();
		let name = || Pull::attr("person/name");
		// Text and builder agree
		let cases: Vec<(&str, Vec<Pull>)> = vec![
			("[:person/name *]", vec![name(), Pull::Wildcard]),
			("[:person/_friend :person/_boss]", vec![Pull::reverse("person/friend"), Pull::reverse("person/boss")]),
			("[[:person/aka :limit 2 :as \"aliases\"] (:person/age :default 0)]", vec![Pull::attr("person/aka").limit(2).key("aliases"), Pull::attr("person/age").default(0)]),
			("[{:person/friend [:person/name]}]", vec![Pull::attr("person/friend").nest(vec![name()])]),
			("[:person/name {[:person/boss :as :chain] ...}]", vec![name(), Pull::attr("person/boss").key("chain").recurse(None)]),
			("[:person/name {:person/boss 2 :person/friend [:person/name]}]", vec![name(), Pull::attr("person/boss").recurse(Some(2)), Pull::attr("person/friend").nest(vec![name()])]),
		];
		for (text, pattern) in &cases
		{
			for &x in &e
			{
				assert_eq!(db.pull_str(text, x).unwrap(), db.pull(pattern, x).unwrap(), "{text}");
			}
		}
		// What the options do
		let p = db.pull_str("[[:person/aka :limit 2 :as \"aliases\"] [:person/age :default 0] [:person/name :default \"?\"]]", e[1]).unwrap();
		assert_eq!(p.get("person/age"), Some(&PullValue::Val(Value::Int(0))));
		assert_eq!(p.get("aliases"), None);
		let p = db.pull_str("[[:person/aka :limit 2 :as \"aliases\"] [:person/age :default 0]]", e[0]).unwrap();
		assert_eq!(p.get("aliases"), Some(&PullValue::Many(vec![PullValue::Val("x".into()), PullValue::Val("y".into())])));
		assert_eq!(p.get("person/age"), Some(&PullValue::Val(Value::Int(30))));
		// Recursion: unbounded down the chain, two levels by a depth, and a cycle ends with a Ref
		fn depth(p: &Pulled, key: &str) -> usize
		{
			match p.get(key)
			{
				Some(PullValue::Entity(q)) => 1 + depth(q, key),
				_ => 0,
			}
		}
		assert_eq!(depth(&db.pull_str("[:person/name {:person/boss ...}]", e[0]).unwrap(), "person/boss"), 3);
		assert_eq!(depth(&db.pull_str("[:person/name {:person/boss 2}]", e[0]).unwrap(), "person/boss"), 2);
		assert!(matches!(db.pull_str("[:person/name {:person/boss 2}]", e[0]).unwrap().get("person/boss"), Some(PullValue::Entity(q)) if matches!(q.get("person/boss"), Some(PullValue::Entity(r)) if r.get("person/boss") == Some(&PullValue::Val(Value::Ref(e[3]))))));
		let p = db.pull_str("[:person/name {:person/friend ...}]", e[0]).unwrap();
		let PullValue::Many(fs) = p.get("person/friend").unwrap() else { panic!() };
		let PullValue::Entity(b) = &fs[0] else { panic!() };
		assert_eq!(b.get("person/friend"), Some(&PullValue::Many(vec![PullValue::Val(Value::Ref(e[0]))])));
		// Errors: syntax with positions, then the pattern's own (an unknown attribute, a default of the wrong type, recursion through a non-Ref)
		for bad in ["[:person/name", "{:person/name}", "[1]", "[[:person/aka :limit]]", "[[:person/aka :color 2]]", "[[:person/aka :limit -1]]", "[{:person/boss :x}]", "[[]]", ":person/name"]
		{
			assert!(matches!(db.pull_str(bad, e[0]), Err(Error::Parse { .. })), "{bad}");
		}
		assert!(matches!(db.pull_str("[:nope]", e[0]), Err(Error::UnknownAttr(_))));
		assert!(matches!(db.pull_str("[[:person/age :default \"old\"]]", e[0]), Err(Error::TypeMismatch { .. })));
		assert!(matches!(db.pull_str("[{:person/name ...}]", e[0]), Err(Error::TypeMismatch { .. })));
		assert!(matches!(db.pull_str("[[:person/_friend :default 1]]", e[0]), Err(Error::InvalidQuery(_))));
		// A map in a query is a syntax error there
		assert!(matches!(db.q("[:find ?e :where {:a 1}]", &[]), Err(Error::Parse { .. })));
	}

	// Floats and instants in text queries: literals, numeric comparison of Ints and Floats, and aggregates
	#[test]
	fn floats_and_instants_in_text()
	{
		let mut conn = Conn::new();
		let mut tx = Tx::new();
		tx.define("item/price", ValueType::Float, Cardinality::One).define("item/n", ValueType::Int, Cardinality::One).define("item/at", ValueType::Instant, Cardinality::One);
		let jan = parse_instant("2024-01-15T00:00:00Z").unwrap();
		let mar = parse_instant("2024-03-01T12:00:00Z").unwrap();
		for (i, (price, at)) in [(1.5, jan), (2.25, jan), (-0.5, mar), (10.0, mar)].into_iter().enumerate()
		{
			let e = tx.temp();
			tx.add(e, "item/price", price).add(e, "item/n", i as i64).add(e, "item/at", Value::Instant(at));
		}
		conn.transact(&tx).unwrap();
		let db = conn.db();
		let q = |text: &str| db.q(text, &[]).unwrap();
		assert_eq!(q("[:find ?p :where [?e :item/price 2.25] [?e :item/price ?p]]"), vec![vec![Value::Float(2.25)]]);
		// An Int compares with a Float numerically (and the Float attribute refuses an Int constant in a pattern)
		assert_eq!(q("[:find ?p :where [?e :item/price ?p] [(> ?p 2)]]"), vec![vec![Value::Float(2.25)], vec![Value::Float(10.0)]]);
		assert_eq!(q("[:find ?n :where [?e :item/price ?p] [?e :item/n ?n] [(= ?p ?n)]]"), Vec::<Vec<Value>>::new());
		assert!(matches!(db.q("[:find ?e :where [?e :item/price 2]]", &[]), Err(Error::TypeMismatch { .. })));
		assert_eq!(q("[:find (sum ?p) (avg ?p) (min ?p) (max ?p) :where [?e :item/price ?p]]"), vec![vec![Value::Float(13.25), Value::Float(3.3125), Value::Float(-0.5), Value::Float(10.0)]]);
		assert_eq!(q("[:find (avg ?n) :where [_ :item/n ?n]]"), vec![vec![Value::Float(1.5)]]);
		// Instants: literals, comparisons, and grouping
		assert_eq!(q("[:find (count ?e) :where [?e :item/at #inst \"2024-01-15\"]]"), vec![vec![Value::Int(2)]]);
		assert_eq!(q("[:find ?n :where [?e :item/at ?t] [(>= ?t #inst \"2024-02-01T00:00:00+01:00\")] [?e :item/n ?n]]"), vec![vec![Value::Int(2)], vec![Value::Int(3)]]);
		assert_eq!(q("[:find ?t (count ?e) :where [?e :item/at ?t]]"), vec![vec![Value::Instant(jan), Value::Int(2)], vec![Value::Instant(mar), Value::Int(2)]]);
		for bad in ["[:find ?e :where [?e :item/at #inst 5]]", "[:find ?e :where [?e :item/at #inst \"tomorrow\"]]", "[:find ?e :where [?e :item/price 1.5.5]]", "[:find ?e :where [?e :item/price 1e]]", "[:find ?e :where #{}]"]
		{
			assert!(matches!(db.q(bad, &[]), Err(Error::Parse { .. })), "{bad}");
		}
	}
}
