//! Who is on the call and which LiveKit identity speaks for each account.

use std::collections::HashMap;

use ring_proto::{AccountKind, internal::RosterMember};

/// The roster plus the authoritative media identity of each account. During a carbon handoff
/// both devices are briefly in the room; only the authoritative one may be heard.
#[derive(Default)]
pub struct Roster {
    members: HashMap<String, RosterMember>,
    authority: HashMap<String, String>,
}

impl Roster {
    pub fn new(members: Vec<RosterMember>) -> Self {
        let mut roster = Self::default();
        roster.replace(members);
        roster
    }

    /// Replaces the member list. Identities from the roster are the server's current truth;
    /// a known identity is kept for members whose roster entry has none yet.
    pub fn replace(&mut self, members: Vec<RosterMember>) {
        self.members.clear();
        for member in members {
            if let Some(identity) = &member.identity {
                self.authority.insert(member.uuid.clone(), identity.clone());
            }
            self.members.insert(member.uuid.clone(), member);
        }
        self.authority
            .retain(|uuid, _| self.members.contains_key(uuid));
    }

    pub fn set_authority(&mut self, account_uuid: &str, identity: &str) {
        self.authority
            .insert(account_uuid.to_owned(), identity.to_owned());
    }

    pub fn members(&self) -> impl Iterator<Item = &RosterMember> {
        self.members.values()
    }

    pub fn carbons(&self) -> impl Iterator<Item = &RosterMember> {
        self.members
            .values()
            .filter(|m| m.kind == AccountKind::Carbon)
    }

    /// Whether audio from `identity` belongs in a mix: for a carbon, only its authoritative
    /// device. Identities the roster does not know yet (a participant who joined moments before
    /// the roster update) are heard.
    pub fn audible(&self, identity: &str) -> bool {
        match carbon_account(identity).and_then(|uuid| self.authority.get(uuid)) {
            Some(authoritative) => authoritative == identity,
            None => true,
        }
    }

    /// The identity currently carrying `account_uuid`'s audio. Falls back to the only device
    /// of that carbon present in the room when the server has not named one yet.
    pub fn resolve<'a>(
        &'a self,
        account_uuid: &str,
        present: impl Iterator<Item = &'a str>,
    ) -> Option<String> {
        if let Some(identity) = self.authority.get(account_uuid) {
            return Some(identity.clone());
        }
        let mut candidates = present.filter(|id| carbon_account(id) == Some(account_uuid));
        let first = candidates.next()?;
        candidates.next().is_none().then(|| first.to_owned())
    }
}

/// `carbon:<uuid>:<device>` → `<uuid>`.
fn carbon_account(identity: &str) -> Option<&str> {
    let rest = identity.strip_prefix("carbon:")?;
    Some(rest.split_once(':').map_or(rest, |(uuid, _)| uuid))
}

/// "Alice (c:alice)" or just the id when there is no display name.
pub fn describe(display_name: Option<&str>, id: &str) -> String {
    match display_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) if name != id => format!("{name} ({id})"),
        _ => id.to_owned(),
    }
}
