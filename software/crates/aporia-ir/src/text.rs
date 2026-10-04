//! Canonical textual form of A-IR.
//!
//! The text form exists for three jobs: it is what a reviewer reads when comparing two models, it
//! is what a golden test diffs, and it is what makes an experiment replayable in ten years when
//! whatever serialised it is gone. So it is line-oriented, fully explicit (every instruction
//! carries its own id and its inferred type), and floats go through bit patterns so that a
//! round trip cannot shift a value by one unit in the last place.

use crate::dim::{Dimension, NumType, Ty};
use crate::ir::{
    Block, BlockId, Builtin, CmpOp, Constraint, ConstraintId, ConstraintKind, Direction, Domain,
    Instr, InstrKind, Lit, Model, Operand, Origin, Output, Param, Relation, RelationId,
    RelationKind, Slot, Trace, Unop,
};
use std::fmt::Write as _;

/// Version marker written on the first line of every dump.
pub const FORMAT: &str = "aporia-ir 1";

/// A textual IR that could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for TextError {}

fn err(line: usize, message: impl Into<String>) -> TextError {
    TextError {
        line,
        message: message.into(),
    }
}

fn num_name(t: NumType) -> &'static str {
    match t {
        NumType::F64 => "f64",
        NumType::F32 => "f32",
        NumType::I64 => "i64",
        NumType::Bool => "bool",
        NumType::Unit => "unit",
    }
}

fn parse_num(s: &str) -> Result<NumType, ()> {
    Ok(match s {
        "f64" => NumType::F64,
        "f32" => NumType::F32,
        "i64" => NumType::I64,
        "bool" => NumType::Bool,
        "unit" => NumType::Unit,
        _ => return Err(()),
    })
}

/// Exact float text: hexadecimal bit pattern, never a decimal approximation.
fn hexf(v: f64) -> String {
    format!("{:#018x}", v.to_bits())
}

fn unhexf(s: &str) -> Result<f64, ()> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16)
        .map(f64::from_bits)
        .map_err(|_| ())
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn unquote(s: &str) -> Result<String, ()> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || bytes[bytes.len() - 1] != b'"' {
        return Err(());
    }
    let mut out = String::new();
    let mut it = s[1..s.len() - 1].chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            _ => return Err(()),
        }
    }
    Ok(out)
}

/// Split a line into fields, keeping double-quoted runs together.
fn fields(line: &str) -> Result<Vec<String>, ()> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    for c in line.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_quote => {
                cur.push(c);
                escaped = true;
            }
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            _ if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if in_quote {
        return Err(());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

fn parse_operand(tok: &str) -> Result<Operand, ()> {
    if let Some(rest) = tok.strip_prefix('p') {
        return Ok(Operand::Param(rest.parse::<u16>().map_err(|_| ())?));
    }
    if let Some(rest) = tok.strip_prefix('s') {
        return Ok(Operand::Slot(rest.parse::<u16>().map_err(|_| ())?));
    }
    if let Some(rest) = tok.strip_prefix('n') {
        return Ok(Operand::Node(rest.parse::<u32>().map_err(|_| ())?));
    }
    if let Some(rest) = tok.strip_prefix("lf64:") {
        return Ok(Operand::Lit(Lit::F64(unhexf(rest)?)));
    }
    if let Some(rest) = tok.strip_prefix("lf32:") {
        let bits = u32::from_str_radix(rest.trim_start_matches("0x"), 16).map_err(|_| ())?;
        return Ok(Operand::Lit(Lit::F32(f32::from_bits(bits))));
    }
    if let Some(rest) = tok.strip_prefix("li64:") {
        return Ok(Operand::Lit(Lit::I64(rest.parse::<i64>().map_err(|_| ())?)));
    }
    if let Some(rest) = tok.strip_prefix("lb:") {
        return Ok(Operand::Lit(Lit::Bool(match rest {
            "true" => true,
            "false" => false,
            _ => return Err(()),
        })));
    }
    Err(())
}

fn parse_domain(fields: &[String]) -> Result<(Domain, usize), ()> {
    match fields.first().map(String::as_str) {
        Some("interval") if fields.len() >= 3 => Ok((
            Domain::Interval {
                lo: unhexf(&fields[1])?,
                hi: unhexf(&fields[2])?,
            },
            3,
        )),
        Some("choices") => {
            let mut vals = Vec::with_capacity(fields.len() - 1);
            for f in &fields[1..] {
                vals.push(unhexf(f)?);
            }
            Ok((Domain::Choices(vals), fields.len()))
        }
        _ => Err(()),
    }
}

fn domain_text(domain: &Domain, out: &mut String) {
    match domain {
        Domain::Interval { lo, hi } => {
            let _ = write!(out, " interval {} {}", hexf(*lo), hexf(*hi));
        }
        Domain::Choices(vals) => {
            let _ = write!(out, " choices");
            for v in vals {
                let _ = write!(out, " {}", hexf(*v));
            }
        }
    }
}

fn origin_name(o: Origin) -> &'static str {
    match o {
        Origin::Declared => "declared",
        Origin::Inferred => "inferred",
    }
}

fn parse_origin(s: &str) -> Result<Origin, ()> {
    Ok(match s {
        "declared" => Origin::Declared,
        "inferred" => Origin::Inferred,
        _ => return Err(()),
    })
}

/// Render a model as canonical text.
///
/// Long on purpose: a serialiser and its parser read best when the whole wire format is visible in
/// one place, so the two functions here are a table rather than a call graph.
#[expect(clippy::too_many_lines)]
#[must_use]
pub fn to_text(model: &Model) -> String {
    let mut s = String::new();
    s.push_str(FORMAT);
    s.push('\n');
    let _ = writeln!(s, "model {} {}", quote(&model.name), quote(&model.doc));
    for (i, p) in model.params.iter().enumerate() {
        let _ = write!(
            s,
            "param {i} {} {} {} {}",
            quote(&p.name),
            num_name(p.ty.num),
            p.ty.dim,
            hexf(p.to_si)
        );
        domain_text(&p.domain, &mut s);
        let _ = writeln!(s, " {}", quote(&p.doc));
    }
    for (i, slot) in model.slots.iter().enumerate() {
        let _ = writeln!(
            s,
            "slot {i} {} {} {} {}",
            quote(&slot.name),
            num_name(slot.ty.num),
            slot.ty.dim,
            slot.init.to_canonical()
        );
    }
    for (bid, block) in model.blocks.iter().enumerate() {
        let _ = writeln!(s, "block {bid}");
        for id in &block.instrs {
            let instr = &model.instrs[*id as usize];
            let _ = write!(s, "instr {id} {} {} ", num_name(instr.ty.num), instr.ty.dim);
            match &instr.kind {
                InstrKind::Bin { op, a, b } => {
                    let _ = writeln!(
                        s,
                        "bin {} {} {}",
                        op.name(),
                        a.to_canonical(),
                        b.to_canonical()
                    );
                }
                InstrKind::Un { op, a } => {
                    let _ = writeln!(s, "un {} {}", op.name(), a.to_canonical());
                }
                InstrKind::Call { builtin, args } => {
                    let joined: Vec<String> = args.iter().map(Operand::to_canonical).collect();
                    let _ = writeln!(s, "call {} {}", builtin.name(), joined.join(","));
                }
                InstrKind::For { trip, body } => {
                    let _ = writeln!(s, "for {} {body}", trip.to_canonical());
                }
                InstrKind::Write { slot, value } => {
                    let _ = writeln!(s, "write s{slot} {}", value.to_canonical());
                }
            }
        }
    }
    for (i, o) in model.outputs.iter().enumerate() {
        let _ = writeln!(
            s,
            "output {i} {} {} {} {}",
            quote(&o.name),
            num_name(o.ty.num),
            o.ty.dim,
            o.value.to_canonical()
        );
        let _ = writeln!(s, "doc output {i} {}", quote(&o.doc));
    }
    for (i, t) in model.traces.iter().enumerate() {
        let _ = writeln!(
            s,
            "trace {i} {} {} {} {} {}",
            quote(&t.name),
            num_name(t.ty.num),
            t.ty.dim,
            t.scope,
            t.value.to_canonical()
        );
    }
    for c in &model.constraints {
        let head = format!(
            "constraint {} {} {}",
            c.id,
            quote(&c.name),
            origin_name(c.origin)
        );
        match &c.kind {
            ConstraintKind::Cmp {
                lhs,
                cmp,
                rhs,
                tolerance,
            } => {
                let _ = writeln!(
                    s,
                    "{head} cmp {} {} {} {}",
                    lhs.to_canonical(),
                    cmp.name(),
                    rhs.to_canonical(),
                    hexf(*tolerance)
                );
            }
            ConstraintKind::Finite { value } => {
                let _ = writeln!(s, "{head} finite {}", value.to_canonical());
            }
        }
    }
    for r in &model.relations {
        let head = format!(
            "relation {} {} {}",
            r.id,
            quote(&r.name),
            origin_name(r.origin)
        );
        match &r.kind {
            RelationKind::Monotone {
                out,
                param,
                direction,
            } => {
                let _ = writeln!(s, "{head} monotone o{out} p{param} {}", direction.name());
            }
            RelationKind::ScalesAs { out, param, power } => {
                let _ = writeln!(s, "{head} scales_as o{out} p{param} {}", hexf(*power));
            }
            RelationKind::Symmetric { out, pair } => {
                let _ = writeln!(s, "{head} symmetric o{out} p{} p{}", pair[0], pair[1]);
            }
            RelationKind::Conserved { trace, tolerance } => {
                let _ = writeln!(s, "{head} conserved t{trace} {}", hexf(*tolerance));
            }
            RelationKind::Lipschitz { out, param, bound } => {
                let _ = writeln!(s, "{head} lipschitz o{out} p{param} {}", hexf(*bound));
            }
        }
    }
    s
}

/// Parse canonical text back into a model.
#[expect(clippy::too_many_lines)]
pub fn from_text(text: &str) -> Result<Model, TextError> {
    let mut model = Model::new(String::new());
    let mut cur_block: Option<BlockId> = None;
    let mut seen_format = false;
    // Instructions are collected by their declared id rather than appended, because a dump is
    // grouped by block and a lowered loop body is numbered before the `For` that enters it.
    let mut placed: Vec<Option<Instr>> = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = lineno + 1;
        let raw = raw.trim_end();
        if raw.is_empty() {
            continue;
        }
        let fs = fields(raw).map_err(|_| err(line, "unbalanced quotes"))?;
        if fs.is_empty() {
            continue;
        }
        match fs[0].as_str() {
            "aporia-ir" => {
                if fs.len() != 2 || fs[1] != "1" {
                    return Err(err(line, "expected `aporia-ir 1`"));
                }
                seen_format = true;
            }
            "model" => {
                if fs.len() != 3 {
                    return Err(err(line, "model takes a name and a doc string"));
                }
                model.name = unquote(&fs[1]).map_err(|_| err(line, "bad model name"))?;
                model.doc = unquote(&fs[2]).map_err(|_| err(line, "bad model doc"))?;
            }
            "param" => {
                // param <id> <name> <num> <dim> <to_si> interval|choices ... <doc>
                if fs.len() < 7 {
                    return Err(err(line, "param line is too short"));
                }
                let id: usize = fs[1].parse().map_err(|_| err(line, "bad param id"))?;
                if id != model.params.len() {
                    return Err(err(line, "parameters must appear in order"));
                }
                let domain_fields: Vec<String> = fs[6..fs.len() - 1].to_vec();
                let (domain, used) =
                    parse_domain(&domain_fields).map_err(|_| err(line, "bad domain"))?;
                let doc_at = 6 + used;
                if doc_at >= fs.len() {
                    return Err(err(line, "param line is missing its doc string"));
                }
                model.params.push(Param {
                    name: unquote(&fs[2]).map_err(|_| err(line, "bad param name"))?,
                    ty: Ty {
                        num: parse_num(&fs[3]).map_err(|_| err(line, "bad param type"))?,
                        dim: Dimension::parse_canonical(&fs[4])
                            .map_err(|_| err(line, "bad param dimension"))?,
                    },
                    domain,
                    to_si: unhexf(&fs[5]).map_err(|_| err(line, "bad param scale"))?,
                    doc: unquote(&fs[doc_at]).map_err(|_| err(line, "bad param doc"))?,
                });
            }
            "slot" => {
                if fs.len() != 6 {
                    return Err(err(line, "slot takes name, num, dim and an initialiser"));
                }
                let id: usize = fs[1].parse().map_err(|_| err(line, "bad slot id"))?;
                if id != model.slots.len() {
                    return Err(err(line, "slots must appear in order"));
                }
                model.slots.push(Slot {
                    name: unquote(&fs[2]).map_err(|_| err(line, "bad slot name"))?,
                    ty: Ty {
                        num: parse_num(&fs[3]).map_err(|_| err(line, "bad slot type"))?,
                        dim: Dimension::parse_canonical(&fs[4])
                            .map_err(|_| err(line, "bad slot dimension"))?,
                    },
                    init: parse_operand(&fs[5]).map_err(|_| err(line, "bad slot initialiser"))?,
                });
            }
            "block" => {
                let id: BlockId = fs[1].parse().map_err(|_| err(line, "bad block id"))?;
                if id == 0 && model.blocks.len() == 1 {
                    // The entry block already exists; a dump always names it.
                    cur_block = Some(0);
                    continue;
                }
                if id as usize != model.blocks.len() {
                    return Err(err(line, "blocks must appear in order"));
                }
                model.blocks.push(Block::default());
                cur_block = Some(id);
            }
            "instr" => {
                let Some(bid) = cur_block else {
                    return Err(err(line, "instruction outside any block"));
                };
                if fs.len() < 5 {
                    return Err(err(line, "instr line is too short"));
                }
                let id: u32 = fs[1].parse().map_err(|_| err(line, "bad instruction id"))?;
                let at = id as usize;
                if placed.get(at).is_some_and(Option::is_some) {
                    return Err(err(line, format!("instruction {id} appears twice")));
                }
                if placed.len() < at + 1 {
                    placed.resize(at + 1, None);
                }
                let ty = Ty {
                    num: parse_num(&fs[2]).map_err(|_| err(line, "bad instruction type"))?,
                    dim: Dimension::parse_canonical(&fs[3])
                        .map_err(|_| err(line, "bad instruction dimension"))?,
                };
                let kind = match fs[4].as_str() {
                    "bin" => {
                        if fs.len() != 8 {
                            return Err(err(line, "bin takes an operator and two operands"));
                        }
                        InstrKind::Bin {
                            op: BinopParse::op(&fs[5]).ok_or_else(|| err(line, "bad binop"))?,
                            a: parse_operand(&fs[6]).map_err(|_| err(line, "bad bin operand"))?,
                            b: parse_operand(&fs[7]).map_err(|_| err(line, "bad bin operand"))?,
                        }
                    }
                    "un" => {
                        if fs.len() != 7 {
                            return Err(err(line, "un takes an operator and one operand"));
                        }
                        InstrKind::Un {
                            op: Unop::from_name(&fs[5]).ok_or_else(|| err(line, "bad unop"))?,
                            a: parse_operand(&fs[6]).map_err(|_| err(line, "bad un operand"))?,
                        }
                    }
                    "call" => {
                        if fs.len() != 7 {
                            return Err(err(line, "call takes a builtin and an operand list"));
                        }
                        let builtin =
                            Builtin::from_name(&fs[5]).ok_or_else(|| err(line, "bad builtin"))?;
                        let args = fs[6]
                            .split(',')
                            .map(parse_operand)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| err(line, "bad call operand"))?;
                        InstrKind::Call { builtin, args }
                    }
                    "for" => {
                        if fs.len() != 7 {
                            return Err(err(line, "for takes a trip count and a block"));
                        }
                        InstrKind::For {
                            trip: parse_operand(&fs[5]).map_err(|_| err(line, "bad trip count"))?,
                            body: fs[6]
                                .parse::<BlockId>()
                                .map_err(|_| err(line, "bad body block"))?,
                        }
                    }
                    "write" => {
                        if fs.len() != 7 {
                            return Err(err(line, "write takes a slot and a value"));
                        }
                        let slot = fs[5]
                            .strip_prefix('s')
                            .and_then(|s| s.parse::<u16>().ok())
                            .ok_or_else(|| err(line, "bad slot reference"))?;
                        InstrKind::Write {
                            slot,
                            value: parse_operand(&fs[6])
                                .map_err(|_| err(line, "bad write value"))?,
                        }
                    }
                    other => return Err(err(line, format!("unknown instruction `{other}`"))),
                };
                placed[at] = Some(Instr { ty, kind });
                model.blocks[bid as usize].instrs.push(id);
            }
            "output" => {
                if fs.len() != 6 {
                    return Err(err(line, "output takes name, num, dim and a value"));
                }
                let id: usize = fs[1].parse().map_err(|_| err(line, "bad output id"))?;
                if id != model.outputs.len() {
                    return Err(err(line, "outputs must appear in order"));
                }
                model.outputs.push(Output {
                    name: unquote(&fs[2]).map_err(|_| err(line, "bad output name"))?,
                    ty: Ty {
                        num: parse_num(&fs[3]).map_err(|_| err(line, "bad output type"))?,
                        dim: Dimension::parse_canonical(&fs[4])
                            .map_err(|_| err(line, "bad output dimension"))?,
                    },
                    value: parse_operand(&fs[5]).map_err(|_| err(line, "bad output value"))?,
                    doc: String::new(),
                });
            }
            "doc" => {
                // doc output <id> <string>
                if fs.len() != 4 || fs[1] != "output" {
                    return Err(err(line, "unexpected doc line"));
                }
                let id: usize = fs[2].parse().map_err(|_| err(line, "bad output id"))?;
                let o = model
                    .outputs
                    .get_mut(id)
                    .ok_or_else(|| err(line, "doc for unknown output"))?;
                o.doc = unquote(&fs[3]).map_err(|_| err(line, "bad output doc"))?;
            }
            "trace" => {
                if fs.len() != 7 {
                    return Err(err(line, "trace takes name, num, dim, scope and a value"));
                }
                let id: usize = fs[1].parse().map_err(|_| err(line, "bad trace id"))?;
                if id != model.traces.len() {
                    return Err(err(line, "traces must appear in order"));
                }
                model.traces.push(Trace {
                    name: unquote(&fs[2]).map_err(|_| err(line, "bad trace name"))?,
                    ty: Ty {
                        num: parse_num(&fs[3]).map_err(|_| err(line, "bad trace type"))?,
                        dim: Dimension::parse_canonical(&fs[4])
                            .map_err(|_| err(line, "bad trace dimension"))?,
                    },
                    scope: fs[5]
                        .parse::<BlockId>()
                        .map_err(|_| err(line, "bad trace scope"))?,
                    value: parse_operand(&fs[6]).map_err(|_| err(line, "bad trace value"))?,
                });
            }
            "constraint" => {
                if fs.len() < 5 {
                    return Err(err(line, "constraint line is too short"));
                }
                let id: ConstraintId = fs[1].parse().map_err(|_| err(line, "bad constraint id"))?;
                let name = unquote(&fs[2]).map_err(|_| err(line, "bad constraint name"))?;
                let origin = parse_origin(&fs[3]).map_err(|_| err(line, "bad origin"))?;
                let kind = match fs[4].as_str() {
                    "cmp" => {
                        if fs.len() != 9 {
                            return Err(err(line, "cmp constraint takes lhs, op, rhs, tolerance"));
                        }
                        ConstraintKind::Cmp {
                            lhs: parse_operand(&fs[5]).map_err(|_| err(line, "bad lhs"))?,
                            cmp: CmpOp::from_name(&fs[6]).ok_or_else(|| err(line, "bad cmp"))?,
                            rhs: parse_operand(&fs[7]).map_err(|_| err(line, "bad rhs"))?,
                            tolerance: unhexf(&fs[8]).map_err(|_| err(line, "bad tolerance"))?,
                        }
                    }
                    "finite" => {
                        if fs.len() != 6 {
                            return Err(err(line, "finite constraint takes one value"));
                        }
                        ConstraintKind::Finite {
                            value: parse_operand(&fs[5]).map_err(|_| err(line, "bad value"))?,
                        }
                    }
                    other => return Err(err(line, format!("unknown constraint kind `{other}`"))),
                };
                model.constraints.push(Constraint {
                    id,
                    name,
                    kind,
                    origin,
                });
            }
            "relation" => {
                if fs.len() < 6 {
                    return Err(err(line, "relation line is too short"));
                }
                let id: RelationId = fs[1].parse().map_err(|_| err(line, "bad relation id"))?;
                let name = unquote(&fs[2]).map_err(|_| err(line, "bad relation name"))?;
                let origin = parse_origin(&fs[3]).map_err(|_| err(line, "bad origin"))?;
                let idx = |tok: &str, c: char| -> Result<u16, TextError> {
                    tok.strip_prefix(c)
                        .and_then(|s| s.parse::<u16>().ok())
                        .ok_or_else(|| err(line, format!("expected {c}<index>, got `{tok}`")))
                };
                let want = match fs[4].as_str() {
                    "conserved" => 7,
                    "monotone" | "scales_as" | "symmetric" | "lipschitz" => 8,
                    _ => 0,
                };
                if want == 0 {
                    return Err(err(line, format!("unknown relation kind `{}`", fs[4])));
                }
                if fs.len() != want {
                    return Err(err(
                        line,
                        format!("relation kind `{}` wants {want} fields", fs[4]),
                    ));
                }
                let kind = match fs[4].as_str() {
                    "monotone" => RelationKind::Monotone {
                        out: idx(&fs[5], 'o')?,
                        param: idx(&fs[6], 'p')?,
                        direction: match fs[7].as_str() {
                            "up" => Direction::Increasing,
                            "down" => Direction::Decreasing,
                            _ => return Err(err(line, "direction must be up or down")),
                        },
                    },
                    "scales_as" => RelationKind::ScalesAs {
                        out: idx(&fs[5], 'o')?,
                        param: idx(&fs[6], 'p')?,
                        power: unhexf(&fs[7]).map_err(|_| err(line, "bad power"))?,
                    },
                    "symmetric" => RelationKind::Symmetric {
                        out: idx(&fs[5], 'o')?,
                        pair: [idx(&fs[6], 'p')?, idx(&fs[7], 'p')?],
                    },
                    "conserved" => RelationKind::Conserved {
                        trace: idx(&fs[5], 't')?,
                        tolerance: unhexf(&fs[6]).map_err(|_| err(line, "bad tolerance"))?,
                    },
                    "lipschitz" => RelationKind::Lipschitz {
                        out: idx(&fs[5], 'o')?,
                        param: idx(&fs[6], 'p')?,
                        bound: unhexf(&fs[7]).map_err(|_| err(line, "bad bound"))?,
                    },
                    other => return Err(err(line, format!("unknown relation kind `{other}`"))),
                };
                model.relations.push(Relation {
                    id,
                    name,
                    kind,
                    origin,
                });
            }
            other => return Err(err(line, format!("unknown keyword `{other}`"))),
        }
    }
    if !seen_format {
        return Err(err(0, "missing `aporia-ir 1` header"));
    }
    // Node operands index into `instrs` by position, so the ids have to form a complete set with no
    // holes; a dump that skipped one would otherwise silently shift every later reference.
    for (i, slot) in placed.into_iter().enumerate() {
        let Some(instr) = slot else {
            return Err(err(0, format!("instruction {i} is missing from the dump")));
        };
        model.instrs.push(instr);
    }
    Ok(model)
}

/// Binop parsing lives here so `ir.rs` stays free of text-format concerns.
struct BinopParse;

impl BinopParse {
    fn op(name: &str) -> Option<crate::Binop> {
        crate::Binop::from_name(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dim::{LENGTH, TIME};
    use crate::ir::{Binop, ConstraintKind, RelationKind};

    /// A model shaped the way lowering shapes a real one: the loop body is numbered before the
    /// `For` that enters it, so a dump grouped by block does not list instruction ids in order.
    fn looped() -> Model {
        let mut m = Model::new("decay");
        m.params.push(Param {
            name: "step".into(),
            ty: Ty::float(Dimension::dimensionless()),
            domain: Domain::interval(0.0, 1.0),
            to_si: 1.0,
            doc: String::new(),
        });
        m.slots.push(Slot {
            name: "e".into(),
            ty: Ty::float(Dimension::dimensionless()),
            init: Operand::Lit(Lit::F64(1.0)),
        });
        let unit = Ty {
            num: NumType::Unit,
            dim: Dimension::dimensionless(),
        };
        let body = m.new_block();
        let decay = m.push(
            body,
            Instr {
                ty: Ty::float(Dimension::dimensionless()),
                kind: InstrKind::Bin {
                    op: Binop::Mul,
                    a: Operand::Slot(0),
                    b: Operand::Lit(Lit::F64(0.5)),
                },
            },
        );
        m.push(
            body,
            Instr {
                ty: unit,
                kind: InstrKind::Write {
                    slot: 0,
                    value: Operand::Node(decay),
                },
            },
        );
        let entrance = m.push_entry(Instr {
            ty: unit,
            kind: InstrKind::For {
                trip: Operand::Lit(Lit::I64(3)),
                body,
            },
        });
        assert_eq!(entrance, 2, "the loop is numbered after its own body");
        m.outputs.push(Output {
            name: "e".into(),
            ty: Ty::float(Dimension::dimensionless()),
            value: Operand::Slot(0),
            doc: String::new(),
        });
        m.traces.push(Trace {
            name: "e".into(),
            ty: Ty::float(Dimension::dimensionless()),
            scope: body,
            value: Operand::Slot(0),
        });
        m
    }

    #[test]
    fn a_dump_grouped_by_block_reads_back_even_when_ids_interleave() {
        let m = looped();
        let text = to_text(&m);
        let back = from_text(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(to_text(&back), text, "the round trip changed the model");
        assert_eq!(back.instrs.len(), m.instrs.len());
        assert_eq!(back.blocks[1].instrs, vec![0, 1]);
        assert_eq!(back.blocks[0].instrs, vec![2]);
    }

    #[test]
    fn an_instruction_declared_twice_is_refused() {
        let text = to_text(&looped());
        // Renumber the entry `For` to collide with the body's first instruction.
        let duplicated = text.replace("instr 2 unit", "instr 0 unit");
        let e = from_text(&duplicated).unwrap_err();
        assert!(e.message.contains("appears twice"), "{e}");
    }

    #[test]
    fn a_dump_with_a_missing_instruction_is_refused() {
        // Node operands index by position, so a hole would silently shift every later reference.
        let text = to_text(&looped());
        let dropped = text
            .lines()
            .filter(|l| !l.starts_with("instr 1 "))
            .collect::<Vec<_>>()
            .join("\n");
        let e = from_text(&dropped).unwrap_err();
        assert!(e.message.contains("missing"), "{e}");
    }

    fn sample() -> Model {
        let mut m = Model::new("pendulum_small");
        m.doc = "single swing estimate".into();
        m.params.push(Param {
            name: "length".into(),
            ty: Ty::float(Dimension::base(LENGTH, 1)),
            domain: Domain::interval(0.1, 4.0),
            to_si: 1.0,
            doc: "wire length".into(),
        });
        m.params.push(Param {
            name: "gravity".into(),
            ty: Ty::float(Dimension::base(LENGTH, 1).div(&Dimension::base(TIME, 1).pow(2))),
            domain: Domain::interval(9.7, 9.9),
            to_si: 1.0,
            doc: String::new(),
        });
        let period = Ty::float(Dimension::base(TIME, 1));
        let ratio = m.push_entry(Instr {
            ty: Ty::float(Dimension::base(TIME, 1)),
            kind: InstrKind::Bin {
                op: Binop::Div,
                a: Operand::Param(0),
                b: Operand::Param(1),
            },
        });
        let scaled = m.push_entry(Instr {
            ty: Ty::float(Dimension::base(TIME, 1)),
            kind: InstrKind::Bin {
                op: Binop::Mul,
                a: Operand::Node(ratio),
                b: Operand::Lit(Lit::F64(std::f64::consts::TAU)),
            },
        });
        m.outputs.push(Output {
            name: "period".into(),
            ty: period,
            value: Operand::Node(scaled),
            doc: "swing period".into(),
        });
        m.constraints.push(Constraint {
            id: 0,
            name: "gravity_positive".into(),
            kind: ConstraintKind::Cmp {
                lhs: Operand::Param(1),
                cmp: CmpOp::Gt,
                rhs: Operand::Lit(Lit::F64(0.0)),
                tolerance: 0.0,
            },
            origin: Origin::Declared,
        });
        m.relations.push(Relation {
            id: 0,
            name: "period_grows_with_length".into(),
            kind: RelationKind::Monotone {
                out: 0,
                param: 0,
                direction: crate::Direction::Increasing,
            },
            origin: Origin::Declared,
        });
        m
    }

    #[test]
    fn text_round_trip_is_exact() {
        let m = sample();
        let text = to_text(&m);
        let back = from_text(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(m, back);
        assert_eq!(to_text(&back), text);
    }

    #[test]
    fn awkward_values_survive_the_round_trip() {
        let mut m = Model::new("awkward");
        m.params.push(Param {
            name: "x".into(),
            ty: Ty::dimensionless_f64(),
            domain: Domain::interval(5e-324, f64::MAX),
            to_si: 1.0,
            doc: String::new(),
        });
        m.outputs.push(Output {
            name: "negzero".into(),
            ty: Ty::dimensionless_f64(),
            value: Operand::Lit(Lit::F64(-0.0)),
            doc: String::new(),
        });
        let t = to_text(&m);
        let back = from_text(&t).unwrap();
        assert_eq!(m, back);
        let Operand::Lit(Lit::F64(v)) = back.outputs[0].value else {
            panic!("expected a literal output");
        };
        assert!(v.is_sign_negative(), "negative zero must stay negative");
    }

    #[test]
    fn a_bad_line_names_its_line_number() {
        let mut text = to_text(&sample());
        text = text.replace("block 0", "block 7");
        let e = from_text(&text).unwrap_err();
        assert_eq!(e.message, "blocks must appear in order");
        assert!(e.line > 1);
    }

    #[test]
    fn unknown_keyword_is_rejected() {
        let e = from_text("aporia-ir 1\nwibble 2\n").unwrap_err();
        assert!(e.message.contains("unknown keyword"));
    }

    #[test]
    fn quoting_handles_the_hard_cases() {
        let odd = "a \"quoted\", back\\slash and\nnewline";
        let q = quote(odd);
        assert_eq!(unquote(&q).unwrap(), odd);
        assert_eq!(fields(&format!("x {q} y")).unwrap().len(), 3);
    }
}
