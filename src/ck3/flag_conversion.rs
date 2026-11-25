//! CK3 1.21 preserves a flag in a variable-to-variable copy, but a scope value
//! saved from that same variable is numeric. Keep the proof local to one effect
//! sequence: two objects (or two invocations) can use the same name differently.

use crate::block::{BV, Block, Comparator, Eq, Field};
use crate::everything::Everything;
use crate::helpers::TigerHashMap;
use crate::lowercase::Lowercase;
use crate::report::{ErrorKey, err};
use crate::token::Token;

fn literal_flag(text: &str) -> bool {
    text.strip_prefix("flag:").is_some_and(|id| !id.is_empty() && !id.contains(['.', '$']))
}

fn variable_path(text: &str) -> Option<&str> {
    let path = text.strip_prefix("this.").unwrap_or(text);
    let (owners, last) = path.rsplit_once('.').unwrap_or(("", path));
    // Treat two reads as the same object only through stable saved scopes and
    // stored pointers. Random scope links can select a new owner on each use.
    let stable = owners.is_empty()
        || owners.split('.').enumerate().all(|(i, step)| {
            step.starts_with("var:") || (i == 0 && (step == "root" || step.starts_with("scope:")))
        });
    (stable && last.starts_with("var:") && last.len() > 4 && !path.contains(['$', '(', ')']))
        .then_some(path)
}

type Flags<'a> = TigerHashMap<String, &'a Token>;
type Saved<'a> = TigerHashMap<&'static str, &'a Token>;

fn scalar<'a>(block: &'a Block, name: &str) -> Option<&'a Token> {
    let mut fields = block.iter_fields().filter(|f| f.0.as_str().eq_ignore_ascii_case(name));
    let Field(_, Comparator::Equals(Eq::Single), BV::Value(value)) = fields.next()? else {
        return None;
    };
    fields.next().is_none().then_some(value)
}

fn simple_assignment(bv: &BV) -> bool {
    let BV::Block(block) = bv else { return true };
    block.iter_fields().all(|Field(key, _, bv)| {
        let BV::Value(value) = bv else { return false };
        let text = value.as_str();
        key.as_str().eq_ignore_ascii_case("name")
            || value.is_number()
            || matches!(text, "yes" | "no" | "this" | "root")
            || literal_flag(text)
            || variable_path(text).is_some()
            || (text.starts_with("scope:") && !text.contains(['.', '$', '(', ')']))
    })
}

fn flag_writes<'a>(bv: &'a BV, flags: &mut Flags<'a>) {
    let name = match bv {
        BV::Block(write) => scalar(write, "name"),
        BV::Value(name) => Some(name),
    };
    let Some(name) = name.filter(|n| !n.as_str().contains(['$', ':', '.'])) else {
        flags.clear();
        return;
    };
    let proof = bv.get_block().and_then(|write| scalar(write, "value")).and_then(|value| {
        if literal_flag(value.as_str()) {
            Some(value)
        } else {
            variable_path(value.as_str()).and_then(|path| flags.get(path).copied())
        }
    });
    // A qualified receiver can alias the current scope. Forget both its old
    // value and any path that traversed that variable.
    let path = format!("var:{name}");
    flags.retain(|known, _| !known.split('.').any(|step| step == path));
    if let Some(proof) = proof {
        flags.insert(path, proof);
    }
}

// Other trigger calls can themselves save or rebind scopes. Stop at the first
// unknown condition rather than attributing later comparisons to an old copy.
fn flag_comparisons(limit: &Block, saved: &mut Saved) -> bool {
    for Field(key, cmp, bv) in limit.iter_fields() {
        if matches!(Lowercase::new(key.as_str()).as_str(), "and" | "or" | "not" | "nor" | "nand")
            && let Some(child) = bv.get_block()
        {
            if !flag_comparisons(child, saved) {
                return false;
            }
        } else if let Some(name) = key.as_str().strip_prefix("scope:")
            && let BV::Value(value) = bv
            && literal_flag(value.as_str())
        {
            if matches!(cmp, Comparator::Equals(_) | Comparator::NotEquals)
                && let Some(copy) = saved.remove(name)
            {
                let msg = format!(
                    "`save_scope_value_as` cannot preserve a flag from `{copy}` for comparison with `{value}`"
                );
                err(ErrorKey::FlagConversion)
                    .msg(msg)
                    .info("Compare the source variable directly or copy it with `set_variable`.")
                    .loc(copy)
                    .loc_msg(key, "saved value compared with a flag here")
                    .push();
            }
        } else if !matches!(key.as_str(), "always" | "has_variable" | "exists") {
            return false;
        }
    }
    true
}

pub(crate) fn validate(block: &Block, caller: &Lowercase, data: &Everything) {
    if !block.iter_fields().any(|f| f.0.as_str().eq_ignore_ascii_case("save_scope_value_as")) {
        return;
    }
    let mut flags = Flags::default();
    let mut saved = Saved::default();
    if matches!(caller.as_str(), "if" | "else_if" | "while")
        && let Some(limit) = block.get_field_block("limit")
    {
        for Field(key, cmp, bv) in limit.iter_fields() {
            // A direct conjunct must be true on entry. A scripted condition
            // later in the limit could still change its receiver.
            if matches!(cmp, Comparator::Equals(Eq::Single | Eq::Double))
                && let BV::Value(value) = bv
                && literal_flag(value.as_str())
                && let Some(path) = variable_path(key.as_str())
            {
                flags.insert(path.to_owned(), value);
            } else if !matches!(key.as_str(), "always" | "has_variable" | "exists") {
                flags.clear();
            }
        }
    }
    for Field(key, cmp, bv) in block.iter_fields() {
        if *cmp != Comparator::Equals(Eq::Single) || data.get_effect(key).is_some() {
            flags.clear();
            saved.clear();
            continue;
        }
        match Lowercase::new(key.as_str()).as_str() {
            "limit" => (),
            "set_variable" if simple_assignment(bv) => flag_writes(bv, &mut flags),
            "save_scope_value_as" => {
                let Some(save) = bv.get_block() else { continue };
                if !simple_assignment(bv) {
                    // An arbitrary script value can evaluate scripted triggers
                    // that rebind an earlier named scope.
                    flags.clear();
                    saved.clear();
                    continue;
                }
                let Some(name) =
                    scalar(save, "name").filter(|n| !n.as_str().contains(['$', ':', '.']))
                else {
                    // Dynamic target names can replace any saved scope.
                    saved.clear();
                    flags.clear();
                    continue;
                };
                let rebound = format!("scope:{name}");
                saved.remove(name.as_str());
                let source = scalar(save, "value")
                    .and_then(|val| variable_path(val.as_str()).map(|path| (val, path)));
                if let Some((value, path)) = source {
                    if let Some(proof) = flags.get(path) {
                        let msg = format!(
                            "`save_scope_value_as` converts the flag in `{value}` to a numeric value"
                        );
                        err(ErrorKey::FlagConversion).msg(msg)
                            .info("A later comparison with `flag:` fails. Compare the variable directly or copy it with `set_variable`.")
                            .loc(value)
                            .loc_msg(*proof, "flag type established here")
                            .push();
                    } else {
                        saved.insert(name.as_str(), value);
                    }
                }
                flags.retain(|known, _| !known.split('.').any(|step| step == rebound));
            }
            // Calls, branches and scope changes can overwrite a variable or
            // rebind its owner. Their bodies get a separate validation pass.
            api => {
                if !saved.is_empty()
                    && matches!(api, "if" | "else_if" | "while")
                    && let Some(limit) =
                        bv.get_block().and_then(|branch| branch.get_field_block("limit"))
                {
                    flag_comparisons(limit, &mut saved);
                }
                flags.clear();
                saved.clear();
            }
        }
    }
}
