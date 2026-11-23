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
//!   `Db::q_rules`) and scalar inputs `?x`, which are bound to `inputs` in order and become constants of the query
//!   (so the planner sees them as bound).
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
//! * Constants: strings (with the escapes `\"`, `\\`, `\n`, `\t` and `\r`), integers and booleans. There is no keyword value type,
//!   so a keyword can only name an attribute.
//!
//! Commas are whitespace and `;` starts a comment. Syntax errors are `Error::Parse`, with the line and column; the errors of the clauses
//! themselves (an unknown attribute, a type mismatch, ...) are those of the builder.

use crate::db::{Db, Error, Value};
use crate::query::{Agg, Clause, Cmp, Find, Query, Term};
use std::collections::{BTreeSet, HashMap};

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
		self.parse(text, None, inputs)
	}

	/// Parse a text query that takes the rule set `rules` (as `%` in its `:in` section; see the module documentation)
	pub fn parse_query_rules(&self, text: &str, rules: &str, inputs: &[Value]) -> Result<TextQuery, Error>
	{
		self.parse(text, Some(rules), inputs)
	}

	fn parse(&self, text: &str, rules: Option<&str>, inputs: &[Value]) -> Result<TextQuery, Error>
	{
		let node = Reader { text, pos: 0 }.read_all()?;
		Parser { text, q: self.query(), inputs: HashMap::new(), fresh: 0, rule_names: HashMap::new() }.query(node, rules, inputs)
	}

	/// Parse and run a text query
	pub fn q(&self, text: &str, inputs: &[Value]) -> Result<Vec<Vec<Value>>, Error>
	{
		self.parse_query(text, inputs)?.run()
	}

	/// Parse and run a text query that takes the rule set `rules`
	pub fn q_rules(&self, text: &str, rules: &str, inputs: &[Value]) -> Result<Vec<Vec<Value>>, Error>
	{
		self.parse_query_rules(text, rules, inputs)?.run()
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

	fn query(mut self, node: Node, rules: Option<&'a str>, inputs: &[Value]) -> Result<TextQuery, Error>
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
		let mut scalars = Vec::new();
		let mut takes_rules = false;
		if let Some((_, _, names)) = input
		{
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
					}
					Form::Vector(_) => return Err(self.err(n, "collection, tuple and relation inputs are not supported (only scalars ?x)")),
					_ => return Err(self.err(n, "an input is $, % or a variable ?x")),
				}
			}
		}
		if scalars.len() != inputs.len()
		{
			return Err(Error::InvalidQuery(match scalars.len()
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
		self.inputs = scalars.into_iter().zip(inputs.iter().cloned()).collect();

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
			Form::Symbol(s) if s == "_" => Ok(self.blank()),
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
		let mut tq = db.parse_query_rules(text, RULES, &[Value::Int(2)]).unwrap();
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
}
