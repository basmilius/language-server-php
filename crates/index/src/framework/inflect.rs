//! The English inflection and the case changes the frameworks derive names with: a table name from a
//! model, a column from a method, a method from a column.

const UNCOUNTABLE: &[&str] = &[
    "audio",
    "bison",
    "cattle",
    "chassis",
    "compensation",
    "coreopsis",
    "data",
    "deer",
    "education",
    "emoji",
    "equipment",
    "evidence",
    "feedback",
    "firmware",
    "fish",
    "furniture",
    "gold",
    "hardware",
    "information",
    "jedi",
    "kin",
    "knowledge",
    "love",
    "metadata",
    "money",
    "moose",
    "news",
    "nutrition",
    "offspring",
    "plankton",
    "pokemon",
    "police",
    "rain",
    "recommended",
    "related",
    "rice",
    "series",
    "sheep",
    "software",
    "species",
    "swine",
    "traffic",
    "aircraft",
    "advice",
    "bread",
    "butter",
    "cheese",
    "electricity",
    "gas",
    "happiness",
    "homework",
    "honey",
    "ice",
    "juice",
    "luggage",
    "milk",
    "music",
    "oil",
    "pepper",
    "progress",
    "research",
    "salt",
    "sand",
    "sugar",
    "toast",
    "water",
    "wine",
    "wood",
    "wool",
];

const IRREGULAR: &[(&str, &str)] = &[
    ("atlas", "atlases"),
    ("axe", "axes"),
    ("beef", "beefs"),
    ("brother", "brothers"),
    ("cafe", "cafes"),
    ("chateau", "chateaux"),
    ("child", "children"),
    ("cookie", "cookies"),
    ("corpus", "corpuses"),
    ("cow", "cows"),
    ("criterion", "criteria"),
    ("curriculum", "curricula"),
    ("demo", "demos"),
    ("domino", "dominoes"),
    ("echo", "echoes"),
    ("foot", "feet"),
    ("fungus", "fungi"),
    ("ganglion", "ganglions"),
    ("gas", "gases"),
    ("genie", "genies"),
    ("genus", "genera"),
    ("goose", "geese"),
    ("graffito", "graffiti"),
    ("hippopotamus", "hippopotami"),
    ("hoof", "hoofs"),
    ("human", "humans"),
    ("iris", "irises"),
    ("larva", "larvae"),
    ("leaf", "leaves"),
    ("lens", "lenses"),
    ("loaf", "loaves"),
    ("man", "men"),
    ("medium", "media"),
    ("memorandum", "memoranda"),
    ("money", "monies"),
    ("mongoose", "mongooses"),
    ("motto", "mottoes"),
    ("move", "moves"),
    ("mythos", "mythoi"),
    ("niche", "niches"),
    ("nucleus", "nuclei"),
    ("numen", "numina"),
    ("occiput", "occiputs"),
    ("octopus", "octopuses"),
    ("opus", "opuses"),
    ("ox", "oxen"),
    ("passerby", "passersby"),
    ("penis", "penises"),
    ("person", "people"),
    ("plateau", "plateaux"),
    ("runner-up", "runners-up"),
    ("safe", "safes"),
    ("sex", "sexes"),
    ("sieve", "sieves"),
    ("soliloquy", "soliloquies"),
    ("son-in-law", "sons-in-law"),
    ("syllabus", "syllabi"),
    ("testis", "testes"),
    ("thief", "thieves"),
    ("tooth", "teeth"),
    ("tornado", "tornadoes"),
    ("trilby", "trilbys"),
    ("turf", "turfs"),
    ("valve", "valves"),
    ("volcano", "volcanoes"),
    ("woman", "women"),
];

/// The plural of an English word, by the rules the frameworks' own inflector follows. A word in
/// capitals keeps them, and a word the rules do not know takes an `s`.
pub fn pluralize(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    if lower.is_empty() || UNCOUNTABLE.contains(&lower.as_str()) {
        return word.to_string();
    }
    if let Some((_, plural)) = IRREGULAR.iter().find(|(singular, _)| *singular == lower) {
        return with_case_of(word, plural);
    }
    if IRREGULAR.iter().any(|(_, plural)| *plural == lower) {
        return word.to_string();
    }
    let cut = |count: usize, suffix: &str| format!("{}{suffix}", &word[..word.len() - count]);
    let ends = |suffix: &str| lower.ends_with(suffix);
    if ends("quiz") {
        return cut(0, "zes");
    }
    if ends("tatus") {
        return cut(0, "es");
    }
    if ends("ouse") && (lower.ends_with("mouse") || lower.ends_with("louse")) {
        return cut(4, "ice");
    }
    for suffix in ["matrix", "vertex", "index"] {
        if ends(suffix) {
            return cut(2, "ices");
        }
    }
    if ends("x") || ends("ch") || ends("ss") || ends("sh") {
        return cut(0, "es");
    }
    if ends("y") && lower.len() > 1 {
        let before = lower.as_bytes()[lower.len() - 2];
        let vowel = matches!(before, b'a' | b'e' | b'i' | b'o' | b'u' | b'y');
        if !vowel || lower.ends_with("quy") {
            return cut(1, "ies");
        }
    }
    if ends("fe") && !ends("ffe") {
        return cut(2, "ves");
    }
    if (ends("lf") || ends("rf")) && !ends("rff") {
        return cut(1, "ves");
    }
    if ends("sis") {
        return cut(2, "es");
    }
    if ends("um") && (ends("ium") || ends("tum") || ends("ium")) {
        return cut(2, "a");
    }
    for suffix in ["buffalo", "tomato", "potato", "hero", "veto"] {
        if ends(suffix) {
            return cut(0, "es");
        }
    }
    if ends("bus") && !ends("abus") {
        return cut(0, "es");
    }
    if ends("alias") {
        return cut(0, "es");
    }
    if ends("octopus") || ends("virus") {
        return cut(2, "i");
    }
    if ends("axis") || ends("testis") {
        return cut(2, "es");
    }
    if ends("s") {
        return word.to_string();
    }
    format!("{word}s")
}

fn with_case_of(word: &str, replacement: &str) -> String {
    if word.chars().next().is_some_and(char::is_uppercase) {
        let mut chars = replacement.chars();
        return match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        };
    }
    replacement.to_string()
}

/// `UserProfile` and `userProfile` as `user_profile`. A run of capitals splits letter by letter,
/// which is what `Str::snake` does.
pub fn snake(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for (position, ch) in text.chars().enumerate() {
        if ch.is_uppercase() {
            if position > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// `user_profile` and `user-profile` as `UserProfile`.
pub fn studly(text: &str) -> String {
    text.split(['_', '-', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `user_profile` as `userProfile`.
pub fn camel(text: &str) -> String {
    let studly = studly(text);
    let mut chars = studly.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The table a model without a `$table` reads: the snake case of its class name with the last word
/// in the plural.
pub fn table_of(class_name: &str) -> String {
    let short = class_name.rsplit('\\').next().unwrap_or(class_name);
    let snake = snake(short);
    match snake.rsplit_once('_') {
        Some((head, last)) => format!("{head}_{}", pluralize(last)),
        None => pluralize(&snake),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pluralizes_as_the_frameworks_do() {
        for (singular, plural) in [
            ("user", "users"),
            ("post", "posts"),
            ("category", "categories"),
            ("day", "days"),
            ("box", "boxes"),
            ("bus", "buses"),
            ("address", "addresses"),
            ("person", "people"),
            ("child", "children"),
            ("status", "statuses"),
            ("quiz", "quizzes"),
            ("knife", "knives"),
            ("wolf", "wolves"),
            ("analysis", "analyses"),
            ("tomato", "tomatoes"),
            ("news", "news"),
            ("sheep", "sheep"),
            ("index", "indices"),
            ("mouse", "mice"),
            ("Person", "People"),
            ("series", "series"),
            ("shoe", "shoes"),
            ("hero", "heroes"),
            ("media", "media"),
            ("data", "data"),
        ] {
            assert_eq!(pluralize(singular), plural, "{singular}");
        }
    }

    #[test]
    fn changes_case() {
        assert_eq!(snake("UserProfile"), "user_profile");
        assert_eq!(snake("userId"), "user_id");
        assert_eq!(snake("APIKey"), "a_p_i_key");
        assert_eq!(studly("first_name"), "FirstName");
        assert_eq!(camel("first_name"), "firstName");
        assert_eq!(table_of("App\\Models\\UserProfile"), "user_profiles");
        assert_eq!(table_of("App\\Models\\Post"), "posts");
        assert_eq!(table_of("Category"), "categories");
    }
}
