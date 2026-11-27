use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub const MAX_NOTES: usize = 200;
pub const MAX_NOTE_TEXT_BYTES: usize = 16 * 1024;
pub const MAX_NOTES_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteSource {
    Player,
    Companion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    id: String,
    text: String,
    source: NoteSource,
}

impl Note {
    pub fn new(id: String, text: String, source: NoteSource) -> Result<Self, String> {
        validate_id(&id)?;
        validate_text(&text)?;
        Ok(Self { id, text, source })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn source(&self) -> NoteSource {
        self.source
    }

    fn numeric_id(&self) -> Option<u64> {
        parse_id(&self.id)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Notes(Vec<Note>);

impl Notes {
    pub fn from_records(records: Vec<Note>) -> Result<Self, String> {
        let notes = Self(records);
        notes.validate()?;
        Ok(notes)
    }
}

/// A turn-local notebook. Mutations are applied to the live notebook only
/// after the turn succeeds.
#[derive(Clone, Debug)]
pub struct NoteWorkspace {
    notes: Notes,
    changes: Vec<NoteChange>,
    next_id: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
enum NoteChange {
    Put(Note),
    Delete(String),
}

impl NoteWorkspace {
    pub fn new(notes: Notes, next_id: Arc<AtomicU64>) -> Self {
        Self {
            notes,
            changes: Vec::new(),
            next_id,
        }
    }

    pub fn notes(&self) -> &Notes {
        &self.notes
    }

    pub fn create(&mut self, text: String) -> Result<Note, String> {
        validate_text(&text)?;
        let id = self
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "The note ID space is exhausted.".to_owned())?;
        let note = Note::new(format!("n{id}"), text, NoteSource::Companion)?;
        self.notes.add(note.clone())?;
        self.changes.push(NoteChange::Put(note.clone()));
        Ok(note)
    }

    pub fn edit(&mut self, id: &str, text: String) -> Result<Note, String> {
        let original = self
            .notes
            .find(id)
            .ok_or_else(|| format!("Note {id} was not found."))?;
        if original.source != NoteSource::Companion {
            return Err("The companion cannot edit player-sourced notes.".to_owned());
        }
        let note = Note::new(id.to_owned(), text, NoteSource::Companion)?;
        let mut candidate = self.notes.clone();
        candidate.remove(id);
        candidate.add(note.clone())?;
        self.notes = candidate;
        self.changes.push(NoteChange::Put(note.clone()));
        Ok(note)
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let note = self
            .notes
            .find(id)
            .ok_or_else(|| format!("Note {id} was not found."))?;
        if note.source != NoteSource::Companion {
            return Err("The companion cannot delete player-sourced notes.".to_owned());
        }
        self.notes.remove(id);
        self.changes.push(NoteChange::Delete(id.to_owned()));
        Ok(())
    }

    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Commits staged operations in order. A later commit wins if the player
    /// changed the same companion note during the turn.
    pub fn commit(&self, current: &Notes) -> Result<Notes, String> {
        let mut result = current.clone();
        for change in &self.changes {
            let id = match change {
                NoteChange::Put(note) => note.id(),
                NoteChange::Delete(id) => id,
            };
            if result
                .find(id)
                .is_some_and(|note| note.source == NoteSource::Player)
            {
                return Err(format!("Cannot overwrite player-sourced note {id}."));
            }
            result.remove(id);
            if let NoteChange::Put(note) = change {
                result.add(note.clone())?;
            }
        }
        Ok(result)
    }
}

impl Notes {
    pub fn iter(&self) -> impl Iterator<Item = &Note> {
        self.0.iter()
    }

    pub fn find(&self, id: &str) -> Option<&Note> {
        self.0.iter().find(|note| note.id == id)
    }

    pub fn add(&mut self, note: Note) -> Result<(), String> {
        note.validate()?;
        if self.find(&note.id).is_some() {
            return Err(format!("Note {} already exists.", note.id));
        }
        if self.0.len() >= MAX_NOTES {
            return Err(format!("The notebook is full (maximum {MAX_NOTES} notes)."));
        }

        let mut candidate = self.0.clone();
        candidate.push(note);
        validate_records(&candidate)?;
        self.0 = candidate;
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Option<Note> {
        let index = self.0.iter().position(|note| note.id == id)?;
        Some(self.0.remove(index))
    }

    pub fn next_id_floor(&self) -> Result<u64, String> {
        let highest = self
            .0
            .iter()
            .map(|note| {
                note.numeric_id()
                    .ok_or_else(|| format!("Invalid note ID {:?}.", note.id))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max();
        highest.map_or(Ok(1), |id| {
            id.checked_add(1)
                .ok_or_else(|| "The note ID space is exhausted.".to_owned())
        })
    }

    /// Keeps player notes from the current state and companion notes from the
    /// state before the rolled-back turn.
    pub fn merged_for_rollback(&self, before: &Notes) -> Notes {
        let mut records: Vec<Note> = self
            .0
            .iter()
            .filter(|note| note.source == NoteSource::Player)
            .cloned()
            .collect();
        for note in before
            .0
            .iter()
            .filter(|note| note.source == NoteSource::Companion)
        {
            if !records.iter().any(|existing| existing.id == note.id) {
                records.push(note.clone());
            }
        }
        records.sort_by_key(|note| note.numeric_id().unwrap_or(u64::MAX));
        Notes(records)
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_records(&self.0)
    }
}

impl Note {
    fn validate(&self) -> Result<(), String> {
        validate_id(&self.id)?;
        validate_text(&self.text)
    }
}

fn validate_records(records: &[Note]) -> Result<(), String> {
    if records.len() > MAX_NOTES {
        return Err(format!(
            "Saved notebook has too many notes ({}; maximum {MAX_NOTES}).",
            records.len()
        ));
    }
    let mut ids = std::collections::HashSet::with_capacity(records.len());
    for note in records {
        note.validate()?;
        if !ids.insert(note.id.as_str()) {
            return Err(format!(
                "Saved notebook contains duplicate note ID {}.",
                note.id
            ));
        }
    }
    let bytes = serde_json::to_vec(records)
        .map_err(|error| format!("Could not serialize notebook: {error}"))?
        .len();
    if bytes > MAX_NOTES_BYTES {
        return Err(format!(
            "Notebook exceeds the {MAX_NOTES_BYTES}-byte limit."
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), String> {
    if parse_id(id).is_none() {
        return Err(format!(
            "Invalid note ID {id:?}; expected n followed by a positive integer."
        ));
    }
    Ok(())
}

fn parse_id(id: &str) -> Option<u64> {
    let number = id.strip_prefix('n')?;
    if number.is_empty() || (number.len() > 1 && number.starts_with('0')) {
        return None;
    }
    let value = number.parse::<u64>().ok()?;
    (value > 0).then_some(value)
}

fn validate_text(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Note text cannot be empty.".to_owned());
    }
    if text.len() > MAX_NOTE_TEXT_BYTES {
        return Err(format!(
            "Note text exceeds the {MAX_NOTE_TEXT_BYTES}-byte limit."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, text: &str, source: NoteSource) -> Note {
        Note::new(id.to_owned(), text.to_owned(), source).unwrap()
    }

    #[test]
    fn should_reject_invalid_or_duplicate_note_ids() {
        for id in ["", "n0", "n01", "note1", "n-1"] {
            assert!(Note::new(id.to_owned(), "text".to_owned(), NoteSource::Player).is_err());
        }

        let mut notes = Notes::default();
        notes.add(note("n1", "first", NoteSource::Player)).unwrap();
        assert!(notes
            .add(note("n1", "duplicate", NoteSource::Player))
            .unwrap_err()
            .contains("already exists"));
    }

    #[test]
    fn should_reject_empty_and_oversized_note_text() {
        assert!(Note::new("n1".to_owned(), " \n".to_owned(), NoteSource::Player).is_err());
        assert!(Note::new(
            "n1".to_owned(),
            "x".repeat(MAX_NOTE_TEXT_BYTES + 1),
            NoteSource::Player
        )
        .is_err());
    }

    #[test]
    fn should_keep_player_notes_and_restore_companion_notes_during_rollback() {
        let before = Notes::from_records(vec![
            note("n1", "companion before", NoteSource::Companion),
            note("n2", "player before", NoteSource::Player),
        ])
        .unwrap();
        let current = Notes::from_records(vec![
            note("n1", "companion edited by player", NoteSource::Companion),
            note("n2", "player updated", NoteSource::Player),
            note("n3", "new player note", NoteSource::Player),
            note("n4", "new companion note", NoteSource::Companion),
        ])
        .unwrap();

        let rolled_back = current.merged_for_rollback(&before);

        assert_eq!(
            rolled_back
                .iter()
                .map(|note| (note.id(), note.text()))
                .collect::<Vec<_>>(),
            vec![
                ("n1", "companion before"),
                ("n2", "player updated"),
                ("n3", "new player note")
            ]
        );
    }

    #[test]
    fn should_advance_the_next_id_after_the_highest_remaining_note() {
        let notes = Notes::from_records(vec![note("n7", "text", NoteSource::Player)]).unwrap();
        assert_eq!(notes.next_id_floor().unwrap(), 8);
        assert_eq!(Notes::default().next_id_floor().unwrap(), 1);
    }

    #[test]
    fn should_apply_the_last_committed_companion_note_edit_after_player_deletion() {
        let current =
            Notes::from_records(vec![note("n1", "before", NoteSource::Companion)]).unwrap();
        let mut workspace = NoteWorkspace::new(current.clone(), Arc::new(AtomicU64::new(2)));
        workspace.edit("n1", "after".to_owned()).unwrap();
        let mut deleted = current;
        deleted.remove("n1");

        let committed = workspace.commit(&deleted).unwrap();

        assert_eq!(committed.find("n1").unwrap().text(), "after");
    }

    #[test]
    fn should_refuse_companion_edits_and_deletions_of_player_notes() {
        let notes = Notes::from_records(vec![note("n1", "player", NoteSource::Player)]).unwrap();
        let mut workspace = NoteWorkspace::new(notes, Arc::new(AtomicU64::new(2)));

        assert!(workspace.edit("n1", "replacement".to_owned()).is_err());
        assert!(workspace.delete("n1").is_err());
        assert!(!workspace.has_changes());
    }

    #[test]
    fn should_consume_ids_without_committing_failed_turn_notes() {
        let allocator = Arc::new(AtomicU64::new(1));
        let current = Notes::default();
        let mut abandoned = NoteWorkspace::new(current.clone(), allocator.clone());
        abandoned.create("discarded".to_owned()).unwrap();
        let mut successful = NoteWorkspace::new(current.clone(), allocator);
        let note = successful.create("retained".to_owned()).unwrap();

        assert_eq!(note.id(), "n2");
        assert!(current.find("n1").is_none());
        assert!(successful.commit(&current).unwrap().find("n2").is_some());
    }
}
