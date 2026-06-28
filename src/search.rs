use crate::note::Note;

/// Return indices into `notes` matching `query`, best match first.
/// Title matches are ranked before content-only matches; ties preserve the
/// original (newest-first) order. An empty query returns every note as-is.
pub fn filter_indices(notes: &[Note], query: &str) -> Vec<usize> {
    let q = query.trim();
    if q.is_empty() {
        return (0..notes.len()).collect();
    }
    let q_lower = q.to_lowercase();
    let mut scored: Vec<(u8, usize)> = notes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let in_title = n.meta.title.to_lowercase().contains(&q_lower);
            let in_content = n.content.to_lowercase().contains(&q_lower);
            if in_title {
                Some((1, i)) // title match → highest rank
            } else if in_content {
                Some((0, i)) // content-only match → lower rank
            } else {
                None
            }
        })
        .collect();
    // Higher score first; stable within the same score (preserves newest-first).
    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_with(title: &str, content: &str) -> Note {
        let mut n = Note::new();
        n.meta.title = title.into();
        n.content = content.into();
        n
    }

    #[test]
    fn empty_query_returns_all_in_order() {
        let notes = vec![note_with("a", ""), note_with("b", "")];
        assert_eq!(filter_indices(&notes, ""), vec![0, 1]);
    }

    #[test]
    fn ranks_title_match_first() {
        let notes = vec![
            note_with("groceries", "milk eggs"),
            note_with("meeting notes", "project plan"),
        ];
        let idx = filter_indices(&notes, "groc");
        assert_eq!(idx.first(), Some(&0));
    }

    #[test]
    fn matches_in_content() {
        let notes = vec![
            note_with("daily", "remember to water the plants"),
            note_with("work", "ship the release"),
        ];
        let idx = filter_indices(&notes, "plants");
        assert_eq!(idx, vec![0]);
    }

    #[test]
    fn non_matching_notes_are_excluded() {
        let notes = vec![
            note_with("meeting notes", "project roadmap"),
            note_with("grocery list", "milk and eggs"),
            note_with("shopping", "buy groceries and soap"),
        ];
        // "grocery list" title contains "grocer"; "shopping" content contains "grocer"
        // (from "groceries"). "meeting notes" does not.
        let idx = filter_indices(&notes, "grocer");
        assert_eq!(idx.len(), 2);
        assert!(!idx.contains(&0)); // "meeting notes" must be absent
    }

    #[test]
    fn title_matches_rank_before_content_matches() {
        let notes = vec![
            note_with("daily log", "todo: call alice"),
            note_with("todo list", "groceries"),
        ];
        // "todo list" is a title match; "daily log" is content-only.
        let idx = filter_indices(&notes, "todo");
        assert_eq!(idx, vec![1, 0]);
    }
}
