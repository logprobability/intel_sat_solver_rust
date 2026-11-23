//! A text syntax for queries: a subset of Datomic's Datalog, written in EDN, read into the `Query` builder.
//!
//! ```text
//! [:find ?name (count ?f)
//!  :with ?x
//!  :in $ ?city
//!  :where [?p :person/city ?city]
//!         [?p :person/name ?name]
//!         [?p :person/follows ?f]
//!         [(> ?age 30)]
//!         (not [?f :person/city ?city])
//!         (not-join [?p] [?p :person/banned true])
//!         (or [?f :person/age 40] (and [?f :person/vip true] [?f :person/age 41]))
//!         (or-join [?p] [?p :person/admin true] [?x :person/boss ?p])]
//! ```
//! * The sections `:find`, `:with`, `:in` and `:where`, each at most once, in any order; `:find` is required.
//! * `:find` takes variables and aggregates `(count ?x)`, `(count-distinct ?x)`, `(sum ?x)`, `(min ?x)`, `(max ?x)`, `(avg ?x)`
//!   (see `Query::find`); `:with` takes variables.
//! * `:in` takes `$` (the database; one at most, and it may be left out) and scalar inputs `?x`, which are bound to `inputs` in order
//!   and become constants of the query (so the planner sees them as bound).
//! * Patterns `[e :attr v tx added]` of 2 to 5 elements (missing positions are ignored); the attribute is a keyword constant.
//!   `_` is a variable of its own at every occurrence. A lookup ref `[:attr value]` names an entity in the entity or value position.
//! * Comparisons `[(op a b)]` with `=`, `!=` (or `not=`), `<`, `<=`, `>` and `>=`.
//! * `(not clause ...)`, `(not-join [?v ...] clause ...)`, `(or branch ...)` and `(or-join [?v ...] branch ...)`, where a branch
//!   is a clause or `(and clause ...)`.
//! * Constants: strings (with the escapes `\"`, `\\`, `\n`, `\t` and `\r`), integers and booleans. There is no keyword value type,
//!   so a keyword can only name an attribute.
//!
//! Commas are whitespace and `;` starts a comment. Syntax errors are `Error::Parse`, with the line and column; the errors of the clauses
//! themselves (an unknown attribute, a type mismatch, ...) are those of the builder.

use crate::db::{Db, Error, Value};
use crate::query::{Agg, Clause, Cmp, Find, Query, Term};
use std::collections::HashMap;

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
	/// Parse a text query (see the module documentation), with `inputs` bound to the scalar inputs of its `:in` section, in order
	pub fn parse_query(&self, text: &str, inputs: &[Value]) -> Result<TextQuery, Error>
	{
		let node = Reader { text, pos: 0 }.read_all()?;
		Parser { text, q: self.query(), inputs: HashMap::new(), fresh: 0 }.query(node, inputs)
	}

	/// Parse and run a text query
	pub fn q(&self, text: &str, inputs: &[Value]) -> Result<Vec<Vec<Value>>, Error>
	{
		self.parse_query(text, inputs)?.run()
	}
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
	Keyword(String),
	Symbol(String),
	Str(String),
	Int(i64),
	Bool(bool),
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
			'[' | '(' =>
			{
				self.pos += 1;
				let close = if c == '[' { ']' } else { ')' };
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
						Some(d @ (']' | ')')) => return Err(error_at(self.text, self.pos, format!("expected {close} (to close the {c} at {}), found {d}", self.place(pos)))),
						Some(_) => items.push(self.read()?),
					}
				}
				if c == '[' { Form::Vector(items) } else { Form::List(items) }
			}
			']' | ')' => return Err(error_at(self.text, pos, format!("unexpected {c}"))),
			'"' => Form::Str(self.string()?),
			'{' | '}' | '#' | '\'' | '`' | '~' | '^' | '@' | '\\' => return Err(error_at(self.text, pos, format!("{c} is not supported in queries (no maps, sets, tags, quotes or characters)"))),
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
					if !token.strip_prefix(['+', '-']).unwrap_or(token).chars().all(|d| d.is_ascii_digit())
					{
						return Err(error_at(self.text, pos, format!("invalid number {token} (only integers are supported)")));
					}
					Form::Int(token.parse().map_err(|_| error_at(self.text, pos, format!("the integer {token} is out of range")))?)
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
struct Parser<'a>
{
	text: &'a str,
	q: Query,
	/// The scalar inputs, by variable name
	inputs: HashMap<String, Value>,
	/// The number of `_` read so far (each is a variable of its own)
	fresh: usize,
}

impl Parser<'_>
{
	fn err(&self, node: &Node, msg: impl Into<String>) -> Error
	{
		error_at(self.text, node.pos, msg)
	}

	fn query(mut self, node: Node, inputs: &[Value]) -> Result<TextQuery, Error>
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

		// The inputs first, as constants of the clauses
		if let Some((_, _, names)) = input
		{
			let mut scalars = Vec::new();
			let mut sources = 0;
			for n in &names
			{
				match &n.form
				{
					Form::Symbol(s) if s == "$" =>
					{
						sources += 1;
						if sources > 1
						{
							return Err(self.err(n, "only one database input ($) is supported"));
						}
					}
					Form::Symbol(s) if s.starts_with('?') =>
					{
						if scalars.contains(s)
						{
							return Err(self.err(n, format!("the input {s} is named twice")));
						}
						scalars.push(s.clone());
					}
					Form::Symbol(s) if s == "%" => return Err(self.err(n, "rules are not supported")),
					Form::Vector(_) => return Err(self.err(n, "collection, tuple and relation inputs are not supported (only scalars ?x)")),
					_ => return Err(self.err(n, "an input is $ or a variable ?x")),
				}
			}
			if scalars.len() != inputs.len()
			{
				return Err(Error::InvalidQuery(format!("the query takes {} inputs, got {}", scalars.len(), inputs.len())));
			}
			self.inputs = scalars.into_iter().zip(inputs.iter().cloned()).collect();
		}
		else if !inputs.is_empty()
		{
			return Err(Error::InvalidQuery(format!("the query takes no inputs, got {}", inputs.len())));
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

	// A variable (an input's value, for an input) or a constant
	fn term(&mut self, node: &Node) -> Result<Term, Error>
	{
		match &node.form
		{
			Form::Symbol(s) if s == "_" =>
			{
				self.fresh += 1;
				Ok(self.q.var(&format!("_{}", self.fresh)))
			}
			Form::Symbol(s) if s.starts_with('?') => Ok(match self.inputs.get(s)
			{
				Some(v) => Term::Val(v.clone()),
				None => self.q.var(s),
			}),
			Form::Symbol(s) => Err(self.err(node, format!("expected a variable or a constant, found the symbol {s}"))),
			Form::Str(s) => Ok(Term::Val(s.as_str().into())),
			Form::Int(i) => Ok(Term::Val((*i).into())),
			Form::Bool(b) => Ok(Term::Val((*b).into())),
			Form::Keyword(k) => Err(self.err(node, format!("the keyword :{k} is not a value (keywords only name attributes)"))),
			Form::Vector(_) | Form::List(_) => Err(self.err(node, "expected a variable or a constant")),
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
					_ => Err(self.err(&items[0], format!("unknown clause {head} (the clause lists are not, not-join, or and or-join)"))),
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
		let mut next = |p: &mut Self| match rest.next()
		{
			Some(t) => t,
			None =>
			{
				p.fresh += 1;
				p.q.var(&format!("_{}", p.fresh))
			}
		};
		let v = next(self);
		if items.len() <= 3
		{
			return self.q.pattern_clause(e, &attr, v);
		}
		let (tx, added) = (next(self), next(self));
		self.q.pattern_tx_clause(e, &attr, v, tx, added)
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
		self.q.not(join.as_deref(), body)
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
		self.q.or(join.as_deref(), branches)
	}
}

#[cfg(test)]
mod tests
{
	use super::*;
	use crate::db::tests::{random_conn, replay};
	use crate::db::{AttrId, Eid};
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
		let mut tq = db.parse_query(text, &[Value::Int(1)]).unwrap();
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
		let got = db.q("[:find ?e, :in ?a $ :where [?e :tag], [?e :ref ?a]]", &[target.2.clone()]).unwrap();
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
		assert_eq!(at("[:find ?e :in $ [?x ...] :where [?e :n ?x]]"), (1, 17));
		assert_eq!(at("[:find ?e :in % :where [?e :n 1]]"), (1, 15));
		assert_eq!(at("[:find ?e :in ?x ?x :where [?e :n 1]]"), (1, 18));
		assert_eq!(at("[:find ?e :where [?e :n ?v] [[:key ?v] :n 1]]"), (1, 36));
		assert_eq!(at("[:find ?e :where [?e :n ?v] [[:key] :n 1]]"), (1, 30));
		assert_eq!(parse_error(&db, "[:find ?e :where [?e :n 1] (or)]").2, "or needs at least one branch");

		// The inputs, and the errors of the builder, which have no position
		assert_eq!(db.parse_query("[:find ?e :in $ ?x :where [?e :n ?x]]", &[]).err(), Some(Error::InvalidQuery("the query takes 1 inputs, got 0".into())));
		assert!(matches!(db.parse_query("[:find ?e :where [?e :n ?x]]", &[Value::Int(1)]), Err(Error::InvalidQuery(_))));
		assert_eq!(db.parse_query("[:find ?e :where [?e :nope 1]]", &[]).err(), Some(Error::UnknownAttr("nope".into())));
		assert!(matches!(db.parse_query("[:find ?e :where [?e :n \"x\"]]", &[]), Err(Error::TypeMismatch { .. })));
		assert!(matches!(db.parse_query("[:find ?e :where [?e :n 1] (or [?e :n 2] [?f :n 3])]", &[]), Err(Error::InvalidQuery(_))));
		assert_eq!(db.q("[:find ?x :where [?e :n 1]]", &[]).err(), Some(Error::Unbound("?x".into())));
		assert_eq!(db.q("[:find ?e :where (not [?e :n 1])]", &[]).err(), Some(Error::Unbound("?e".into())));
		// An input in a join list is a constant, which drops out of the list
		let tq = db.parse_query("[:find ?e :in ?x :where [?e :n ?x] (not-join [?x] [?e :tag \"a\"])]", &[Value::Int(1)]).unwrap();
		assert!(matches!(&tq.query.clauses()[1], Clause::Not { join, .. } if join.is_empty()));
	}
}
