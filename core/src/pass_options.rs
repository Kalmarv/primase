//! A pass's options declared once ([`PassSpec`]): each option's key, whether a pipeline must give it, an example value
//! and what it means ([`OptionSpec`]). The registry derives the rest from the declaration
//! ([`crate::pass::PassRegistry::add`]): it refuses an undeclared key, a missing required option and a value outside a
//! [`Choice`]'s, each in one form, before the pass is made; it makes the pass from the examples to read its declarations
//! (a pass with a required option is never run so); and it checks at registration that the pass made from the
//! examples prints back what it was made with. The pass reads what the declaration let through ([`Given`]): a required
//! option is there, a choice is its enum.
//!
//! What stays the pass's: parsing and checking a value (numbers, `;` lists, values the data names), and how options
//! constrain each other beyond one-of groups (all four of a rectangle or none; one value that needs another).

use std::collections::BTreeSet;

use crate::pass::{Options, Pass};

/// The values of an option that are a fixed set of names, listed once: the registry refuses any other and names these;
/// the pass reads the enum ([`Given::choice`]), never the text, so there is no second list to drift from.
pub trait Choice: Copy + PartialEq + 'static {
    /// Each value's text and the value; the first is the option's example.
    const VALUES: &'static [(&'static str, Self)];

    /// The text of `self`, as a pass prints it back ([`Pass::options`]).
    fn text(self) -> &'static str {
        Self::VALUES
            .iter()
            .find(|(_, v)| *v == self)
            .map(|(t, _)| *t)
            .expect("every value of a Choice is listed in its VALUES")
    }
}

/// Whether a pipeline must give an option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    /// It must: nothing is assumed for the caller.
    Required,
    /// Exactly one of the options sharing this group name must be given.
    OneOf(&'static str),
    /// It may: its absence has the meaning its doc states (never a choice made for the caller: CONTRIBUTING "Drift").
    Optional,
}

/// One option a pass takes: its key, whether it is needed, an example value and what it means. Built only by its
/// constructors, so a required option always has its example.
#[derive(Clone, Copy, Debug)]
pub struct OptionSpec {
    key: &'static str,
    need: Need,
    example: &'static str,
    doc: &'static str,
    /// The texts of a [`Choice`]'s values, for a choice option.
    values: Option<fn() -> Vec<&'static str>>,
}

fn texts<T: Choice>() -> Vec<&'static str> {
    T::VALUES.iter().map(|(t, _)| *t).collect()
}

impl OptionSpec {
    /// An option a pipeline must give.
    pub const fn required(key: &'static str, example: &'static str, doc: &'static str) -> Self {
        OptionSpec {
            key,
            need: Need::Required,
            example,
            doc,
            values: None,
        }
    }

    /// An option a pipeline may give; `doc` says what its absence means. Its example only documents it: optional
    /// options may exclude each other (`at=centre` or `east` and `north`), so the registry makes no pass from them.
    pub const fn optional(key: &'static str, example: &'static str, doc: &'static str) -> Self {
        OptionSpec {
            key,
            need: Need::Optional,
            example,
            doc,
            values: None,
        }
    }

    /// One of the options of `group`, of which a pipeline gives exactly one.
    pub const fn one_of(
        group: &'static str,
        key: &'static str,
        example: &'static str,
        doc: &'static str,
    ) -> Self {
        OptionSpec {
            key,
            need: Need::OneOf(group),
            example,
            doc,
            values: None,
        }
    }

    /// A required option whose value is one of `T`'s; the example is the first.
    pub const fn choice<T: Choice>(key: &'static str, doc: &'static str) -> Self {
        OptionSpec {
            key,
            need: Need::Required,
            example: T::VALUES[0].0,
            doc,
            values: Some(texts::<T>),
        }
    }

    /// An optional option whose value, when given, is one of `T`'s; `doc` says what its absence means.
    pub const fn optional_choice<T: Choice>(key: &'static str, doc: &'static str) -> Self {
        OptionSpec {
            key,
            need: Need::Optional,
            example: T::VALUES[0].0,
            doc,
            values: Some(texts::<T>),
        }
    }

    pub fn key(&self) -> &'static str {
        self.key
    }

    pub fn example(&self) -> &'static str {
        self.example
    }

    pub fn doc(&self) -> &'static str {
        self.doc
    }

    /// Whether a pipeline must give it (alone, or as one of its group).
    pub fn is_needed(&self) -> bool {
        self.need != Need::Optional
    }

    /// The group it is one of, if it is.
    pub fn group(&self) -> Option<&'static str> {
        match self.need {
            Need::OneOf(g) => Some(g),
            _ => None,
        }
    }

    /// The texts of its values, for a choice option.
    pub fn values(&self) -> Option<Vec<&'static str>> {
        self.values.map(|f| f())
    }
}

/// `items` as `a`, `a or b`, `a, b or c`.
fn alternatives(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// A pass as the registry knows it: its name, the options it takes, and how it is made from what the declaration let
/// through.
pub struct PassSpec {
    pub name: &'static str,
    pub options: &'static [OptionSpec],
    pub make: fn(&Given) -> Result<Box<dyn Pass>, String>,
}

impl std::fmt::Debug for PassSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassSpec")
            .field("name", &self.name)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl PassSpec {
    fn declared(&self, key: &str) -> Option<&OptionSpec> {
        self.options.iter().find(|o| o.key == key)
    }

    /// The groups of one-of options, in the order they are first declared, each with its members.
    fn groups(&self) -> Vec<(&'static str, Vec<&OptionSpec>)> {
        let mut groups: Vec<(&'static str, Vec<&OptionSpec>)> = Vec::new();
        for o in self.options {
            if let Need::OneOf(g) = o.need {
                match groups.iter_mut().find(|(name, _)| *name == g) {
                    Some((_, members)) => members.push(o),
                    None => groups.push((g, vec![o])),
                }
            }
        }
        groups
    }

    /// Whether a pipeline must give it options: a required option or a one-of group.
    pub fn requires_options(&self) -> bool {
        self.options.iter().any(OptionSpec::is_needed)
    }

    /// The options it is made with to read its declarations: each required option's example and the first option's of
    /// each group.
    pub fn example(&self) -> Options {
        let mut seen = BTreeSet::new();
        self.options
            .iter()
            .filter(|o| match o.need {
                Need::Required => true,
                Need::OneOf(g) => seen.insert(g),
                Need::Optional => false,
            })
            .map(|o| (o.key.to_owned(), o.example.to_owned()))
            .collect()
    }

    /// `options` against the declaration: the refusal of the first undeclared key, missing required option, one-of
    /// group given none or more than one of, or choice value outside its values.
    pub fn check(&self, options: &Options) -> Result<(), String> {
        if let Some(k) = options.keys().find(|k| self.declared(k).is_none()) {
            let keys: Vec<&str> = self.options.iter().map(|o| o.key).collect();
            return Err(if keys.is_empty() {
                format!("unknown option {k} ({} takes none)", self.name)
            } else {
                format!("unknown option {k} ({} takes {})", self.name, keys.join(", "))
            });
        }
        if let Some(o) = self
            .options
            .iter()
            .find(|o| o.need == Need::Required && !options.contains_key(o.key))
        {
            return Err(format!("{} is required: {}", o.key, o.doc));
        }
        for (_, members) in self.groups() {
            let keys: Vec<&str> = members.iter().map(|o| o.key).collect();
            match members.iter().filter(|o| options.contains_key(o.key)).count() {
                0 => {
                    let docs: Vec<String> = members.iter().map(|o| format!("{}: {}", o.key, o.doc)).collect();
                    return Err(format!(
                        "one of {} is required ({})",
                        alternatives(&keys),
                        docs.join("; ")
                    ));
                }
                1 => {}
                _ => return Err(format!("only one of {} may be given", alternatives(&keys))),
            }
        }
        for (k, v) in options {
            if let Some(values) = self.declared(k).and_then(OptionSpec::values)
                && !values.contains(&v.as_str())
            {
                return Err(format!("{k}={v} is none of {}", alternatives(&values)));
            }
        }
        Ok(())
    }

    /// The pass made from `options`, once [`PassSpec::check`] lets them through.
    pub fn instance(&self, options: &Options) -> Result<Box<dyn Pass>, String> {
        self.check(options)?;
        (self.make)(&Given { spec: self, options })
    }

    /// Its text form and what each option means: `translate-world{by*}` (`*` a required option, `|` between the
    /// options of a group), then a line per option.
    pub fn describe(&self) -> String {
        let mut out = String::from(self.name);
        if !self.options.is_empty() {
            let mut keys: Vec<String> = Vec::new();
            for o in self.options {
                match o.need {
                    Need::Required => keys.push(format!("{}*", o.key)),
                    Need::Optional => keys.push(o.key.to_owned()),
                    Need::OneOf(g) => {
                        if self
                            .groups()
                            .iter()
                            .any(|(name, m)| *name == g && m[0].key == o.key)
                        {
                            let members: Vec<&str> = self
                                .options
                                .iter()
                                .filter(|m| m.need == Need::OneOf(g))
                                .map(|m| m.key)
                                .collect();
                            keys.push(format!("({})*", members.join("|")));
                        }
                    }
                }
            }
            out.push_str(&format!("{{{}}}", keys.join(",")));
        }
        for o in self.options {
            let need = match o.need {
                Need::Required => "required".to_owned(),
                Need::OneOf(g) => format!("one of {g}"),
                Need::Optional => "optional".to_owned(),
            };
            let values = o
                .values()
                .map(|v| format!("; {}", v.join(" | ")))
                .unwrap_or_default();
            out.push_str(&format!(
                "\n  {} ({need}{values}; e.g. {}): {}",
                o.key, o.example, o.doc
            ));
        }
        out
    }

    /// Panics where the declaration itself is wrong: a key twice, a group of one, a choice with no values or a value
    /// twice.
    pub(crate) fn validate(&self) {
        let mut keys = BTreeSet::new();
        for o in self.options {
            assert!(
                keys.insert(o.key),
                "pass {}: option {} declared twice",
                self.name,
                o.key
            );
            if let Some(values) = o.values() {
                let distinct: BTreeSet<&str> = values.iter().copied().collect();
                assert!(
                    !values.is_empty() && distinct.len() == values.len(),
                    "pass {}: option {}: a choice's values are none or one twice: {values:?}",
                    self.name,
                    o.key
                );
            }
        }
        for (g, members) in self.groups() {
            assert!(
                members.len() > 1,
                "pass {}: the one-of group {g} has one option",
                self.name
            );
        }
    }
}

/// The options a pass is made from, after its declaration let them through ([`PassSpec::check`]). Reading a key the
/// declaration does not have, or reading it as what it is not declared as, is a bug in the pass: a panic.
#[derive(Debug)]
pub struct Given<'a> {
    spec: &'a PassSpec,
    options: &'a Options,
}

impl<'a> Given<'a> {
    fn declared(&self, key: &str) -> &'a OptionSpec {
        self.spec.declared(key).unwrap_or_else(|| {
            panic!(
                "pass {} reads option {key}, which its declaration does not have",
                self.spec.name
            )
        })
    }

    /// A required option's value: always there.
    pub fn required(&self, key: &str) -> &'a str {
        let o = self.declared(key);
        assert!(
            o.need == Need::Required,
            "pass {} reads option {key} as required, which it declares optional or one of a group",
            self.spec.name
        );
        self.options
            .get(key)
            .map(String::as_str)
            .expect("the declaration's check lets no required option be missing")
    }

    /// An optional option's value, or a one-of option's, if given.
    pub fn optional(&self, key: &str) -> Option<&'a str> {
        self.declared(key);
        self.options.get(key).map(String::as_str)
    }

    fn of<T: Choice>(&self, key: &str, text: &str) -> T {
        T::VALUES
            .iter()
            .find(|(t, _)| *t == text)
            .map(|(_, v)| *v)
            .unwrap_or_else(|| {
                panic!(
                    "pass {} reads option {key} as a choice its declaration does not give it",
                    self.spec.name
                )
            })
    }

    /// A required choice's value.
    pub fn choice<T: Choice>(&self, key: &str) -> T {
        let text = self.required(key);
        self.of(key, text)
    }

    /// An optional choice's value, if given.
    pub fn optional_choice<T: Choice>(&self, key: &str) -> Option<T> {
        self.optional(key).map(|text| self.of(key, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;
    use crate::pass::PassLog;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Detail {
        Plain,
        High,
    }

    impl Choice for Detail {
        const VALUES: &'static [(&'static str, Self)] = &[("plain", Detail::Plain), ("high", Detail::High)];
    }

    /// A pass that prints back what it was made with.
    #[derive(Debug)]
    struct Toy(Options);

    impl Pass for Toy {
        fn name(&self) -> &'static str {
            "toy"
        }
        fn options(&self) -> Options {
            self.0.clone()
        }
        fn run(&self, _w: &mut World, _log: &mut PassLog) -> Result<(), String> {
            Ok(())
        }
    }

    const TOY: PassSpec = PassSpec {
        name: "toy",
        options: &[
            OptionSpec::required("cell", "4", "metres, above 0: the grid's cell"),
            OptionSpec::choice::<Detail>("detail", "which collision is written"),
            OptionSpec::one_of("by", "factor", "2", "the enlargement"),
            OptionSpec::one_of("by", "size", "64", "the larger side to reach"),
            OptionSpec::optional("halo", "1", "metres around each cell; none without it"),
            OptionSpec::optional_choice::<Detail>("fallback", "no fallback without it"),
        ],
        make: |g| {
            // every way of reading what the declaration let through
            let _ = (
                g.required("cell"),
                g.choice::<Detail>("detail"),
                g.optional("halo"),
            );
            let _ = (
                g.optional("factor"),
                g.optional("size"),
                g.optional_choice::<Detail>("fallback"),
            );
            Ok(Box::new(Toy(Options::new())))
        },
    };

    fn options(items: &[(&str, &str)]) -> Options {
        items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// The example is each required option's and the first of the group's; a choice's example is its first value.
    #[test]
    fn the_examples_come_from_the_declaration() {
        TOY.validate();
        assert!(TOY.requires_options());
        assert_eq!(
            TOY.example(),
            options(&[("cell", "4"), ("detail", "plain"), ("factor", "2")])
        );
        assert!(TOY.instance(&TOY.example()).is_ok());
    }

    /// Each refusal in its one form, before the pass is made.
    #[test]
    fn the_declaration_refuses_before_the_pass_is_made() {
        let refusal = |items: &[(&str, &str)]| TOY.check(&options(items)).unwrap_err();
        let base = [("cell", "4"), ("detail", "plain"), ("factor", "2")];
        let with = |extra: (&'static str, &'static str)| {
            let mut o = base.to_vec();
            o.push(extra);
            o
        };
        assert_eq!(
            refusal(&with(("colour", "red"))),
            "unknown option colour (toy takes cell, detail, factor, size, halo, fallback)"
        );
        assert_eq!(
            refusal(&[("detail", "plain"), ("factor", "2")]),
            "cell is required: metres, above 0: the grid's cell"
        );
        assert_eq!(
            refusal(&[("cell", "4"), ("detail", "plain")]),
            "one of factor or size is required (factor: the enlargement; size: the larger side to reach)"
        );
        assert_eq!(
            refusal(&with(("size", "64"))),
            "only one of factor or size may be given"
        );
        assert_eq!(
            refusal(&[("cell", "4"), ("detail", "fine"), ("factor", "2")]),
            "detail=fine is none of plain or high"
        );
        assert_eq!(
            refusal(&with(("fallback", "low"))),
            "fallback=low is none of plain or high"
        );
        let none = PassSpec {
            name: "bare",
            options: &[],
            make: |_| Ok(Box::new(Toy(Options::new()))),
        };
        assert_eq!(
            none.check(&options(&[("x", "1")])).unwrap_err(),
            "unknown option x (bare takes none)"
        );
        assert!(!none.requires_options());
    }

    /// A choice reads back as its enum and prints as its text.
    #[test]
    fn a_choice_is_read_as_its_enum() {
        const READ: PassSpec = PassSpec {
            name: "read",
            options: &[OptionSpec::choice::<Detail>("detail", "which")],
            make: |g| {
                let detail = g.choice::<Detail>("detail");
                Ok(Box::new(Toy(options(&[("detail", detail.text())]))))
            },
        };
        let pass = READ.instance(&options(&[("detail", "high")])).unwrap();
        assert_eq!(pass.options(), options(&[("detail", "high")]));
        assert_eq!(Detail::High.text(), "high");
    }

    /// The text form marks required options and groups, and each option's line says its need, values and example.
    #[test]
    fn a_declaration_describes_its_options() {
        let text = TOY.describe();
        assert!(
            text.starts_with("toy{cell*,detail*,(factor|size)*,halo,fallback}\n"),
            "{text}"
        );
        assert!(
            text.contains("\n  detail (required; plain | high; e.g. plain): which collision is written"),
            "{text}"
        );
        assert!(
            text.contains("\n  size (one of by; e.g. 64): the larger side to reach"),
            "{text}"
        );
    }

    /// Reading an option the declaration does not have is a bug in the pass.
    #[test]
    #[should_panic(expected = "pass bad reads option other, which its declaration does not have")]
    fn reading_an_undeclared_option_panics() {
        const BAD: PassSpec = PassSpec {
            name: "bad",
            options: &[OptionSpec::optional("one", "1", "nothing")],
            make: |g| {
                let _ = g.optional("other");
                Ok(Box::new(Toy(Options::new())))
            },
        };
        let _ = BAD.instance(&Options::new());
    }

    /// A group of one is a wrong declaration.
    #[test]
    #[should_panic(expected = "pass lonely: the one-of group by has one option")]
    fn a_group_of_one_is_refused() {
        const LONELY: PassSpec = PassSpec {
            name: "lonely",
            options: &[OptionSpec::one_of("by", "factor", "2", "the enlargement")],
            make: |_| Ok(Box::new(Toy(Options::new()))),
        };
        LONELY.validate();
    }
}
