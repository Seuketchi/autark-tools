//! How contacts are ordered, found and labelled. Nothing here touches the network or a window.

use autark_pim::vcard::Contact;

/// What a contact is called in a list: the name, else the first address or number, else a placeholder.
pub fn title(contact: &Contact) -> String {
    [contact.name.trim(), contact.emails.first().map_or("", String::as_str), contact.phones.first().map_or("", String::as_str), contact.organisation.trim()]
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or("No name")
        .to_owned()
}

/// Sorts by family name, then given name, ignoring case: "Ada Lovelace" files under L.
pub fn sort_key(contact: &Contact) -> String {
    let title = title(contact);
    let (given, family) = title.rsplit_once(' ').unwrap_or(("", &title));
    format!("{} {}", family.to_lowercase(), given.to_lowercase())
}

/// The letter (or `#`) a contact is filed under.
pub fn section(contact: &Contact) -> char {
    sort_key(contact).chars().find(|c| !c.is_whitespace()).filter(|c| c.is_alphabetic()).map_or('#', |c| c.to_uppercase().next().unwrap_or('#'))
}

/// One or two capitals for the round picture: "Ada Lovelace" is AL.
pub fn initials(contact: &Contact) -> String {
    let title = title(contact);
    let mut letters: Vec<char> = title.split_whitespace().filter_map(|word| word.chars().find(|c| c.is_alphanumeric())).flat_map(char::to_uppercase).collect();
    if letters.len() > 2 {
        letters = vec![letters[0], letters[letters.len() - 1]];
    }
    letters.into_iter().collect()
}

/// Whether every word of `query` is somewhere in the contact's name, organisation, addresses or numbers.
pub fn matches(contact: &Contact, query: &str) -> bool {
    let haystack = format!("{} {} {} {}", contact.name, contact.organisation, contact.emails.join(" "), contact.phones.join(" ")).to_lowercase();
    // Numbers match with or without spaces and dashes.
    let digits: String = haystack.chars().filter(char::is_ascii_digit).collect();
    query.split_whitespace().all(|word| {
        let word = word.to_lowercase();
        haystack.contains(&word) || (word.chars().all(|c| c.is_ascii_digit() || c == '+' || c == '-') && word.chars().filter(char::is_ascii_digit).count() >= 3 && digits.contains(&word.chars().filter(char::is_ascii_digit).collect::<String>()))
    })
}

/// A contact from the editor: one address or number per line, blank lines dropped.
pub fn build(uid: &str, name: &str, organisation: &str, emails: &str, phones: &str, note: &str) -> Contact {
    let lines = |text: &str| text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect::<Vec<_>>();
    Contact { uid: uid.to_owned(), name: name.trim().to_owned(), emails: lines(emails), phones: lines(phones), organisation: organisation.trim().to_owned(), note: note.trim().to_owned(), extra: Vec::new() }
}

pub fn valid_email(text: &str) -> bool {
    text.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.') && !text.contains(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(name: &str, email: &str) -> Contact {
        Contact { name: name.into(), emails: if email.is_empty() { vec![] } else { vec![email.into()] }, ..Default::default() }
    }

    #[test]
    fn titles_fall_back_and_sort_by_family_name() {
        assert_eq!(title(&contact("", "a@x.org")), "a@x.org");
        assert_eq!(title(&Contact::default()), "No name");
        let mut people = vec![contact("Grace Hopper", ""), contact("ada lovelace", ""), contact("Alan Turing", "")];
        people.sort_by_key(sort_key);
        assert_eq!(people.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["Grace Hopper", "ada lovelace", "Alan Turing"]);
        assert_eq!((section(&people[1]), section(&contact("", "9@x.org"))), ('L', '#'));
        assert_eq!(initials(&contact("Ada Lovelace", "")), "AL");
        assert_eq!(initials(&contact("Mary Ann Evans Cross", "")), "MC");
        assert_eq!(initials(&contact("Prince", "")), "P");
    }

    #[test]
    fn search_finds_names_addresses_and_numbers() {
        let ada = Contact { name: "Ada Lovelace".into(), organisation: "Analytical Ltd".into(), emails: vec!["ada@example.com".into()], phones: vec!["+44 20 7946 0000".into()], ..Default::default() };
        for query in ["", "ada", "LOVE ltd", "example.com", "7946", "+44 20", "020-7946"] {
            // "020-7946" has a leading zero the stored number lacks, so it is the one that must miss.
            assert_eq!(matches(&ada, query), query != "020-7946", "{query}");
        }
        assert!(!matches(&ada, "grace") && !matches(&ada, "ada grace"));
    }

    #[test]
    fn the_editor_makes_contacts() {
        let made = build("u", " Ada ", "", "a@x.org\n\n b@x.org \n", "123", "  hi ");
        assert_eq!((made.emails.len(), made.phones.len(), made.note.as_str(), made.name.as_str()), (2, 1, "hi", "Ada"));
        assert!(valid_email("a@b.co") && !valid_email("a@b") && !valid_email("a b@c.de") && !valid_email("@c.de"));
    }
}
