//! Fuzzy matching for the command palette.

/// Scores how well `query` matches `candidate` as a subsequence, ignoring case: higher is
/// better, `None` if it doesn't match. Consecutive characters, the start of words and the
/// start of the candidate score extra. Also returns the matched character indices.
pub fn score(query: &str, candidate: &str) -> Option<(i32, Vec<usize>)> {
    let query: Vec<char> = query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let mut positions = Vec::with_capacity(query.len());
    let mut score = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in lower.iter().enumerate() {
        if qi < query.len() && c == query[qi] {
            let word_start = i == 0 || !chars[i - 1].is_alphanumeric();
            score += 1;
            if last.is_some_and(|l| l + 1 == i) {
                score += 5;
            }
            if word_start {
                score += 8;
            }
            if i == 0 {
                score += 4;
            }
            positions.push(i);
            last = Some(i);
            qi += 1;
        }
    }
    if qi < query.len() {
        return None;
    }
    // Shorter candidates win ties.
    Some((score * 4 - chars.len() as i32 / 8, positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_and_ranking() {
        assert!(score("sv", "File: Save").is_some());
        assert!(score("xyz", "File: Save").is_none());
        let (word_starts, _) = score("fs", "File: Save").unwrap();
        let (scattered, _) = score("fs", "Preferences: Open Settings").unwrap();
        assert!(word_starts > scattered);
        assert_eq!(score("save", "File: Save").unwrap().1, vec![6, 7, 8, 9]);
        // Equally good matches: the shorter title first.
        let all = score("select all", "Selection: Select All").unwrap().0;
        let occurrences = score("select all", "Selection: Select All Occurrences")
            .unwrap()
            .0;
        assert!(all > occurrences);
    }
}
