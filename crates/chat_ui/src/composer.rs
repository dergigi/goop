use nostr_sdk::prelude::PublicKey;

pub(super) fn placeholder(members: &[PublicKey], owner: Option<PublicKey>, name: &str) -> String {
    if owner.is_some_and(|owner| members == [owner]) {
        return "Write a note to your future self".into();
    }
    if let (Some(owner), Ok(goop)) = (owner, PublicKey::parse(state::GOOP_NPUB)) {
        // Room membership includes the sender as well as the recipient.
        if owner != goop && members.contains(&goop)
            && members.iter().all(|member| *member == owner || *member == goop)
        {
            return "Report a bug, suggest a feature, or simply say hi".into();
        }
    }
    format!("Message {name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chat::Room;
    use nostr_sdk::prelude::Keys;

    #[test]
    fn goop_direct_message_uses_custom_placeholder_in_either_member_order() {
        let owner = Keys::generate().public_key();
        let goop = PublicKey::parse(state::GOOP_NPUB).unwrap();
        for room in [Room::new(owner, [goop]), Room::new(goop, [owner])] {
            assert_eq!(placeholder(room.members(), Some(owner), "Goop"),
                "Report a bug, suggest a feature, or simply say hi");
        }
    }

    #[test]
    fn groups_and_similarly_named_people_do_not_use_goop_placeholder() {
        let owner = Keys::generate().public_key();
        let other = Keys::generate().public_key();
        let goop = PublicKey::parse(state::GOOP_NPUB).unwrap();
        let group = Room::new(owner, [goop, other]);
        assert_eq!(placeholder(group.members(), Some(owner), "Group"), "Message Group");
        let person = Room::new(owner, [other]);
        assert_eq!(placeholder(person.members(), Some(owner), "Goop"), "Message Goop");
        assert_eq!(placeholder(&[owner], Some(owner), "Me"), "Write a note to your future self");
        assert_eq!(placeholder(&[goop], Some(goop), "Goop"), "Write a note to your future self");
    }
}
