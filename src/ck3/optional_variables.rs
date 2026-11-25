//! Caller preconditions for scoped variables declared optional by the mod's configuration.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

use crate::block::{BV, Block, Comparator, Eq, Field};
use crate::context::ScopeContext;
use crate::report::{ErrorKey, warn};
use crate::token::{Loc, Token};
use crate::{Everything, Item};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Receiver {
    Input,
    Root,
    Frame(isize),
    Named(usize),
    Unknown,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Flow {
    pub(crate) script: Option<Loc>,
    present: Vec<(Receiver, &'static str)>,
    removed: Vec<&'static str>,
}

impl Flow {
    fn establish(&mut self, owner: Receiver, name: &'static str) {
        if owner != Receiver::Unknown && !self.present.contains(&(owner, name)) {
            self.present.push((owner, name));
        }
    }

    fn remove(&mut self, name: &'static str) {
        self.present.retain(|&(_, var)| var != name);
        if !self.removed.contains(&name) {
            self.removed.push(name);
        }
    }

    pub(crate) fn saved_presence(&self) -> Vec<(Receiver, &'static str)> {
        self.present.clone()
    }

    pub(crate) fn leave_branch(&mut self, before: &[(Receiver, &'static str)]) {
        self.present.retain(|fact| before.contains(fact));
    }
}

#[derive(Debug, Default)]
pub(crate) struct OptionalVariables {
    names: BTreeSet<&'static str>,
    pub(crate) removals: BTreeMap<Loc, BTreeSet<&'static str>>,
    observations: Mutex<Observations>,
}

#[derive(Debug, Default)]
struct Observations {
    callers: BTreeMap<Loc, BTreeSet<Call>>,
    reads: BTreeSet<Read>,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Call {
    site: Loc,
    name: &'static str,
    caller: Option<Loc>,
    inherits: bool,
    present: Vec<&'static str>,
    removed: Vec<&'static str>,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Read {
    loc: Loc,
    name: &'static str,
    script: Option<Loc>,
    inherits: bool,
}

fn guard_names(block: &Block, visit: &mut impl FnMut(&'static str)) {
    for Field(key, cmp, bv) in block.iter_fields() {
        if *cmp != Comparator::Equals(Eq::Single) {
            continue;
        }
        if let BV::Value(name) = bv {
            if key.lowercase_is("has_variable") {
                visit(name.as_str());
            } else if key.lowercase_is("exists")
                && let Some(name) = name.as_str().strip_prefix("var:")
                && !name.contains('.')
            {
                visit(name);
            }
        } else if key.lowercase_is("and")
            && let BV::Block(and) = bv
        {
            guard_names(and, visit);
        }
    }
}

impl OptionalVariables {
    pub(crate) fn configure(&mut self, config: &Block) {
        if let Some(names) = config.get_field_block("optional_variables") {
            self.names.extend(names.iter_values_warn().map(Token::as_str));
        }
    }

    /// Preserve caller guards across read-only effects. A conditional removal is
    /// enough to invalidate a proof; an unexpanded effect can contain a dynamic removal.
    pub(crate) fn effect_removals(
        &self,
        data: &Everything,
    ) -> BTreeMap<Loc, BTreeSet<&'static str>> {
        let mut removals = BTreeMap::<Loc, BTreeSet<_>>::new();
        if self.names.is_empty() {
            return removals;
        }
        let mut callers = BTreeMap::<Loc, BTreeSet<Loc>>::new();
        let mut queue = VecDeque::new();
        let local = data
            .events
            .local_script_keys()
            .filter_map(|(kind, key)| (kind == Item::ScriptedEffect).then_some(key));
        for key in data.effects.iter_keys().chain(local) {
            let Some(effect) = data.get_effect(key) else { continue };
            if effect.key.loc != key.loc {
                continue;
            }
            if effect.block.source.is_some() {
                queue.extend(self.names.iter().map(|&name| (key.loc, name)));
                continue;
            }
            let mut blocks = vec![&effect.block];
            while let Some(block) = blocks.pop() {
                for Field(api, _, bv) in block.iter_fields() {
                    if let Some(callee) = data.get_effect(api) {
                        callers.entry(callee.key.loc).or_default().insert(key.loc);
                    } else if api.lowercase_is("remove_variable") {
                        if let Some(name) = bv.get_value().map(Token::as_str)
                            && self.names.contains(name)
                        {
                            queue.push_back((key.loc, name));
                        }
                    } else if let BV::Block(child) = bv {
                        blocks.push(child);
                    }
                }
            }
        }
        while let Some((id, name)) = queue.pop_front() {
            if removals.entry(id).or_default().insert(name)
                && let Some(sites) = callers.get(&id)
            {
                queue.extend(sites.iter().map(|&caller| (caller, name)));
            }
        }
        removals
    }

    pub(crate) fn after_effect(&self, sc: &mut ScopeContext, callee: Loc) {
        if let Some(names) = self.removals.get(&callee) {
            for &name in names {
                sc.optional.remove(name);
            }
        }
    }

    pub(crate) fn guard(&self, sc: &mut ScopeContext, branch: &Block, field: &str) {
        if self.names.is_empty() {
            return;
        }
        if let Some((_, limit)) =
            branch.iter_definitions().filter(|(key, _)| key.lowercase_is(field)).last()
        {
            let owner = sc.variable_receiver();
            guard_names(limit, &mut |name| {
                if self.names.contains(name) {
                    sc.optional.establish(owner, name);
                }
            });
        }
    }

    pub(crate) fn write(&self, sc: &mut ScopeContext, bv: &BV, remove: bool) {
        let name = bv.get_value().or_else(|| bv.get_block()?.get_field_value("name"));
        if let Some(name) = name.map(Token::as_str).filter(|name| self.names.contains(name)) {
            if remove {
                sc.optional.remove(name);
            } else {
                sc.optional.establish(sc.variable_receiver(), name);
            }
        }
    }

    pub(crate) fn read(&self, sc: &ScopeContext, token: &Token, name: &'static str) {
        if !self.names.contains(name) {
            return;
        }
        let owner = sc.variable_receiver();
        if sc.optional.present.contains(&(owner, name)) {
            return;
        }
        self.observations.lock().unwrap().reads.insert(Read {
            loc: token.loc,
            name,
            script: sc.optional.script,
            inherits: owner == Receiver::Input && !sc.optional.removed.contains(&name),
        });
    }

    pub(crate) fn call(&self, sc: &ScopeContext, key: &Token, callee: Loc) {
        if self.names.is_empty() {
            return;
        }
        let owner = sc.variable_receiver();
        let call = Call {
            site: key.loc,
            name: key.as_str(),
            caller: sc.optional.script,
            inherits: owner == Receiver::Input,
            present: sc
                .optional
                .present
                .iter()
                .filter_map(|&(receiver, name)| (receiver == owner).then_some(name))
                .collect(),
            removed: sc.optional.removed.clone(),
        };
        self.observations.lock().unwrap().callers.entry(callee).or_default().insert(call);
    }

    /// Resolve after validation so a recursive or concurrent cache hit cannot publish
    /// an unfinished callee precondition. Each concrete caller is checked separately.
    pub(crate) fn report(&self) {
        let observations = self.observations.lock().unwrap();
        for read in &observations.reads {
            let message = format!("scoped variable `var:{}` may be unset when read", read.name);
            let report = || {
                warn(ErrorKey::OptionalVariable)
                    .msg(&message)
                    .info("Check has_variable on the same object, or use a script value that supplies a fallback.")
            };
            let Some(script) = read.script else {
                report().loc(read.loc).push();
                continue;
            };
            let mut queue = VecDeque::from([(script, read.inherits)]);
            let mut seen = BTreeSet::new();
            while let Some((script, inherits)) = queue.pop_front() {
                if !seen.insert((script, inherits)) {
                    continue;
                }
                let Some(calls) = observations.callers.get(&script) else { continue };
                for call in calls {
                    if inherits && call.present.contains(&read.name) {
                        continue;
                    }
                    if let Some(parent) = call.caller {
                        queue.push_back((
                            parent,
                            inherits && call.inherits && !call.removed.contains(&read.name),
                        ));
                    } else {
                        report()
                            .loc_msg(
                                call.site,
                                format!("call to `{}` can reach the unguarded read", call.name),
                            )
                            .loc_msg(read.loc, "unguarded variable read")
                            .push();
                    }
                }
            }
        }
    }
}
