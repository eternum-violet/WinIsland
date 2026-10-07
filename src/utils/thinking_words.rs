use once_cell::sync::Lazy;
use std::sync::Mutex;

static THINKING_WORDS: Lazy<Mutex<Vec<String>>> = Lazy::new(|| Mutex::new(load_thinking_words()));

fn load_thinking_words() -> Vec<String> {
    let words_content = include_str!("../../words.txt");

    words_content
        .lines()
        .skip(1) // Skip header
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 1 {
                Some(parts[0].to_string()) // English word
            } else {
                None
            }
        })
        .collect()
}

pub fn get_random_thinking_word() -> String {
    let words = THINKING_WORDS.lock().unwrap();

    if words.is_empty() {
        return "Thinking...".to_string();
    }

    let index = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        / 2000) as usize
        % words.len();

    format!("{}...", words[index])
}
