//! The flow's version: a hash of what it does, not how it is laid out.
//!
//! Every expression and value is hashed as temple formats it. That text
//! parses back to the same tree, so layout and comments inside a unit drop
//! out and nothing else does. Temple keeps its formatting stable within a
//! major version, and the hash relies on that: a temple release that changed
//! it would change the hash of every flow. Declarations whose order doesn't
//! matter (entries, events, params, helpers and nodes) are hashed in name
//! order, and annotations not at all. The encoding tags each alternative and
//! prefixes each text with its length, so no two flows share a byte stream.

use crate::common::versions::SEMANTICS_VERSION;
use crate::flow::{Branch, Flow, Helper, Node, Outcome, Param, HASH_LEN};
use sha2::{Digest, Sha256};
use temple_dsl::{ExprUnit, Span, Template, TemplateUnit, Value};

/// What comes next, where the encoding has alternatives. A tag is hashed as
/// its position here, so new tags go last.
#[derive(Clone, Copy)]
#[repr(u8)]
enum Tag {
    Absent,
    Present,
    Else,
    Guard,
    Jump,
    Value,
    Ask,
    Ignore,
    Formatted,
    AsWritten,
    Null,
    Bool,
    Int,
    Decimal,
    Str,
    List,
    Object,
}

pub(super) fn of(flow: &Flow, src: &str) -> [u8; HASH_LEN] {
    let mut hash = Hasher {
        sha: Sha256::new(),
        src,
    };
    hash.number(SEMANTICS_VERSION.into());
    hash.number(flow.edition.into());
    hash.text(&flow.name);
    hash.sorted_texts(flow.entries.iter().map(|&id| flow.node_name(id)).collect());
    hash.sorted_texts(flow.events.iter().map(|event| event.as_str()).collect());

    let mut params: Vec<&Param> = flow.params.iter().collect();
    params.sort_by(|a, b| a.name.cmp(&b.name));
    hash.count(params.len());
    for param in params {
        hash.text(&param.name);
        hash.value(&param.default);
    }
    // each let sees the ones before it, so their order counts
    hash.count(flow.lets.len());
    for binding in &flow.lets {
        hash.text(&binding.name);
        hash.expression(&binding.expression);
    }
    let mut helpers: Vec<&Helper> = flow.helpers.iter().collect();
    helpers.sort_by(|a, b| a.name.cmp(&b.name));
    hash.count(helpers.len());
    for helper in helpers {
        hash.text(&helper.name);
        hash.texts(helper.params.iter().map(|param| param.as_str()));
        hash.expression(&helper.body);
    }
    let mut nodes: Vec<&Node> = flow.nodes.iter().collect();
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    hash.count(nodes.len());
    for node in nodes {
        hash.node(flow, node);
    }
    hash.sha.finalize().into()
}

struct Hasher<'s> {
    sha: Sha256,
    src: &'s str,
}

impl Hasher<'_> {
    fn node(&mut self, flow: &Flow, node: &Node) {
        self.text(&node.name);
        self.texts(node.params.iter().map(|param| param.as_str()));
        self.count(node.lets.len());
        for binding in &node.lets {
            self.text(&binding.name);
            self.expression(&binding.expression);
        }
        self.count(node.branches.len());
        for branch in &node.branches {
            self.branch(flow, branch);
        }
    }

    fn branch(&mut self, flow: &Flow, branch: &Branch) {
        match &branch.guard {
            Some(guard) => {
                self.tag(Tag::Guard);
                self.expression(guard);
            }
            None => self.tag(Tag::Else),
        }
        match &branch.outcome {
            Outcome::Jump { node, args } => {
                self.tag(Tag::Jump);
                self.text(flow.node_name(*node));
                self.expressions(args);
            }
            Outcome::Value { template, wake } => {
                self.tag(Tag::Value);
                self.template(template);
                self.optional(wake.as_ref());
            }
            Outcome::Ask {
                name,
                payload,
                node,
                args,
            } => {
                self.tag(Tag::Ask);
                self.text(name);
                self.optional(payload.as_ref());
                self.text(flow.node_name(*node));
                self.expressions(args);
            }
            Outcome::Ignore => self.tag(Tag::Ignore),
        }
    }

    fn expressions(&mut self, units: &[ExprUnit]) {
        self.count(units.len());
        for unit in units {
            self.expression(unit);
        }
    }

    fn optional(&mut self, unit: Option<&ExprUnit>) {
        match unit {
            Some(unit) => {
                self.tag(Tag::Present);
                self.expression(unit);
            }
            None => self.tag(Tag::Absent),
        }
    }

    /// An expression, formatted as the value of a template's `let`, where
    /// temple parses as deep as in the flow; a hole would take one level
    /// more. The line break keeps `null`, the template's output, clear of
    /// anything the expression ends with.
    fn expression(&mut self, unit: &ExprUnit) {
        let text = &self.src[unit.span().range()];
        let formatted = Template::format(&format!("let e = {text}\nnull"));
        self.formatted(unit.span(), formatted);
    }

    fn template(&mut self, unit: &TemplateUnit) {
        let formatted = Template::format(&self.src[unit.span().range()]);
        self.formatted(unit.span(), formatted);
    }

    fn formatted<E>(&mut self, span: Span, formatted: Result<String, E>) {
        match formatted {
            Ok(canonical) => {
                self.tag(Tag::Formatted);
                self.text(&canonical);
            }
            // temple formats what it parsed, and units are held to half its
            // size limit; were it ever to refuse one, it is hashed as written
            Err(_) => {
                let src = self.src;
                self.tag(Tag::AsWritten);
                self.text(&src[span.range()]);
            }
        }
    }

    fn value(&mut self, value: &Value) {
        match value {
            Value::Null => self.tag(Tag::Null),
            Value::Bool(flag) => {
                self.tag(Tag::Bool);
                self.sha.update([u8::from(*flag)]);
            }
            Value::Int(number) => {
                self.tag(Tag::Int);
                self.sha.update(number.to_le_bytes());
            }
            Value::Decimal(number) => {
                self.tag(Tag::Decimal);
                self.sha.update([u8::from(number.is_sign_negative())]);
                self.sha.update(number.mantissa().to_le_bytes());
                self.sha.update(number.scale().to_le_bytes());
            }
            Value::Str(text) => {
                self.tag(Tag::Str);
                self.text(text);
            }
            Value::Arr(items) => {
                self.tag(Tag::List);
                self.count(items.len());
                for item in items {
                    self.value(item);
                }
            }
            Value::Obj(object) => {
                self.tag(Tag::Object);
                self.count(object.len());
                for (key, item) in object {
                    self.text(key);
                    self.value(item);
                }
            }
        }
    }

    fn sorted_texts(&mut self, mut texts: Vec<&str>) {
        texts.sort_unstable();
        self.texts(texts.into_iter());
    }

    fn texts<'t>(&mut self, texts: impl ExactSizeIterator<Item = &'t str>) {
        self.count(texts.len());
        for text in texts {
            self.text(text);
        }
    }

    fn text(&mut self, text: &str) {
        self.count(text.len());
        self.sha.update(text.as_bytes());
    }

    fn count(&mut self, count: usize) {
        self.number(count as u64);
    }

    fn number(&mut self, number: u64) {
        self.sha.update(number.to_le_bytes());
    }

    fn tag(&mut self, tag: Tag) {
        self.sha.update([tag as u8]);
    }
}
