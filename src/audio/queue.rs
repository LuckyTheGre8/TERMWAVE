//! Playback queue, track navigation, and playback history tracking.
//!
//! Manages ordered track lists, next/previous track resolution,
//! bounded visited history, and queue boundary validation.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use super::AudioError;

/// Maximum number of entries retained in navigation and playback history.
const MAX_HISTORY_ENTRIES: usize = 1024;

/// Ordered track queue with bounded navigation and playback history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackQueue {
    tracks: Vec<PathBuf>,
    current_index: Option<usize>,
    back_stack: VecDeque<usize>,
    played_history: VecDeque<usize>,
}

impl TrackQueue {
    /// Creates a new queue from an ordered collection of local track paths.
    ///
    /// Starts at index `0` if non-empty and records the initial track in playback history.
    pub fn new(tracks: impl IntoIterator<Item = PathBuf>) -> Self {
        let tracks: Vec<PathBuf> = tracks.into_iter().collect();
        let mut played_history = VecDeque::new();
        let current_index = if tracks.is_empty() {
            None
        } else {
            played_history.push_back(0);
            Some(0)
        };

        Self {
            tracks,
            current_index,
            back_stack: VecDeque::new(),
            played_history,
        }
    }

    /// Creates a new queue starting at `start_index`.
    ///
    /// Returns [`AudioError::InvalidState`] if the queue is empty or `start_index`
    /// is out of bounds.
    pub fn with_start_index(
        tracks: impl IntoIterator<Item = PathBuf>,
        start_index: usize,
    ) -> Result<Self, AudioError> {
        let tracks: Vec<PathBuf> = tracks.into_iter().collect();
        if tracks.is_empty() {
            return Err(AudioError::InvalidState(
                "cannot initialize playback queue with no tracks".to_string(),
            ));
        }
        if start_index >= tracks.len() {
            return Err(AudioError::InvalidState(format!(
                "starting track index {start_index} is out of bounds (queue length: {})",
                tracks.len()
            )));
        }

        let mut played_history = VecDeque::new();
        played_history.push_back(start_index);

        Ok(Self {
            tracks,
            current_index: Some(start_index),
            back_stack: VecDeque::new(),
            played_history,
        })
    }

    /// Returns the total number of tracks in the queue.
    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Returns `true` if the queue contains no tracks.
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Returns all tracks in queue order.
    pub fn tracks(&self) -> &[PathBuf] {
        &self.tracks
    }

    /// Returns the 0-based index of the active track, if any.
    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    /// Returns the path of the active track, if any.
    pub fn current_track(&self) -> Option<&Path> {
        self.current_index
            .and_then(|idx| self.tracks.get(idx))
            .map(PathBuf::as_path)
    }

    /// Returns `true` if there is a track after the active track.
    pub fn has_next(&self) -> bool {
        self.current_index
            .is_some_and(|idx| idx + 1 < self.tracks.len())
    }

    /// Returns `true` if navigating to a previous track is possible.
    pub fn has_previous(&self) -> bool {
        !self.back_stack.is_empty() || self.current_index.is_some_and(|idx| idx > 0)
    }

    /// Returns the path of the next upcoming track without advancing the queue cursor.
    pub fn peek_next(&self) -> Option<&Path> {
        self.current_index
            .and_then(|idx| idx.checked_add(1))
            .and_then(|idx| self.tracks.get(idx))
            .map(PathBuf::as_path)
    }

    /// Returns the 0-based index of the next upcoming track, if any.
    pub fn peek_next_index(&self) -> Option<usize> {
        self.current_index
            .and_then(|idx| idx.checked_add(1))
            .filter(|&idx| idx < self.tracks.len())
    }

    /// Returns the path of the previous track in history without modifying the queue cursor.
    pub fn peek_previous(&self) -> Option<&Path> {
        if let Some(&prev) = self.back_stack.back() {
            self.tracks.get(prev).map(PathBuf::as_path)
        } else {
            self.current_index
                .and_then(|idx| idx.checked_sub(1))
                .and_then(|idx| self.tracks.get(idx))
                .map(PathBuf::as_path)
        }
    }

    /// Returns the remaining upcoming tracks after the active track.
    pub fn remaining_tracks(&self) -> &[PathBuf] {
        match self.current_index {
            Some(idx) if idx + 1 < self.tracks.len() => &self.tracks[idx + 1..],
            _ => &[],
        }
    }

    /// Returns `true` if indices `a` and `b` represent consecutive positions in the queue.
    pub fn is_consecutive(&self, a: usize, b: usize) -> bool {
        a.checked_add(1) == Some(b) && b < self.tracks.len()
    }

    /// Advances to the next track in the queue and records the current track
    /// in playback history.
    ///
    /// Returns [`AudioError::InvalidState`] if the queue is empty or already
    /// at the last track.
    pub fn advance_next(&mut self) -> Result<&Path, AudioError> {
        let Some(current) = self.current_index else {
            return Err(AudioError::InvalidState(
                "cannot advance to next track: queue is empty".to_string(),
            ));
        };

        let next = current + 1;
        if next >= self.tracks.len() {
            return Err(AudioError::InvalidState(format!(
                "already at the last track ({}/{})",
                current + 1,
                self.tracks.len()
            )));
        }

        push_bounded(&mut self.back_stack, current);
        push_bounded(&mut self.played_history, next);
        self.current_index = Some(next);
        Ok(self.tracks[next].as_path())
    }

    /// Moves back to the previous track using visited playback history (or the
    /// preceding queue position if started past index `0`).
    ///
    /// Returns [`AudioError::InvalidState`] if the queue is empty or already at
    /// the start boundary with no previous history.
    pub fn step_previous(&mut self) -> Result<&Path, AudioError> {
        let Some(current) = self.current_index else {
            return Err(AudioError::InvalidState(
                "cannot move to previous track: queue is empty".to_string(),
            ));
        };

        let prev = if let Some(from_history) = self.back_stack.pop_back() {
            from_history
        } else if let Some(preceding) = current.checked_sub(1) {
            preceding
        } else {
            return Err(AudioError::InvalidState(
                "already at the first track (no previous track in history)".to_string(),
            ));
        };

        push_bounded(&mut self.played_history, prev);
        self.current_index = Some(prev);
        Ok(self.tracks[prev].as_path())
    }

    /// Returns the indices currently held on the back-navigation stack.
    pub fn back_indices(&self) -> Vec<usize> {
        self.back_stack.iter().copied().collect()
    }

    /// Returns the paths currently held on the back-navigation stack.
    pub fn back_history(&self) -> Vec<PathBuf> {
        self.back_stack
            .iter()
            .filter_map(|&idx| self.tracks.get(idx).cloned())
            .collect()
    }

    /// Returns the chronological indices of tracks entered during this session.
    pub fn played_indices(&self) -> Vec<usize> {
        self.played_history.iter().copied().collect()
    }

    /// Returns the chronological paths of tracks entered during this session.
    pub fn played_history(&self) -> Vec<PathBuf> {
        self.played_history
            .iter()
            .filter_map(|&idx| self.tracks.get(idx).cloned())
            .collect()
    }
}

fn push_bounded(deque: &mut VecDeque<usize>, index: usize) {
    if deque.len() >= MAX_HISTORY_ENTRIES {
        deque.pop_front();
    }
    deque.push_back(index);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tracks() -> Vec<PathBuf> {
        vec![
            PathBuf::from("track1.flac"),
            PathBuf::from("track2.flac"),
            PathBuf::from("track3.flac"),
        ]
    }

    #[test]
    fn empty_queue_enforces_boundaries() {
        let mut queue = TrackQueue::new(Vec::new());
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.current_index(), None);
        assert_eq!(queue.current_track(), None);
        assert!(!queue.has_next());
        assert!(!queue.has_previous());
        assert!(matches!(
            queue.advance_next(),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            queue.step_previous(),
            Err(AudioError::InvalidState(_))
        ));
    }

    #[test]
    fn navigates_next_previous_and_respects_boundaries() {
        let mut queue = TrackQueue::new(sample_tracks());
        assert_eq!(queue.current_index(), Some(0));
        assert_eq!(queue.current_track(), Some(Path::new("track1.flac")));
        assert!(!queue.has_previous());
        assert!(queue.has_next());

        // Cannot go before track 0
        assert!(matches!(
            queue.step_previous(),
            Err(AudioError::InvalidState(_))
        ));
        assert_eq!(queue.current_index(), Some(0));

        // 0 -> 1 -> 2
        assert_eq!(queue.advance_next().unwrap(), Path::new("track2.flac"));
        assert!(queue.has_previous());
        assert!(queue.has_next());

        assert_eq!(queue.advance_next().unwrap(), Path::new("track3.flac"));
        assert!(queue.has_previous());
        assert!(!queue.has_next());

        // Cannot go past track 2
        assert!(matches!(
            queue.advance_next(),
            Err(AudioError::InvalidState(_))
        ));
        assert_eq!(queue.current_index(), Some(2));
        assert_eq!(queue.back_indices(), vec![0, 1]);

        // 2 -> 1 -> 0
        assert_eq!(queue.step_previous().unwrap(), Path::new("track2.flac"));
        assert_eq!(queue.back_indices(), vec![0]);

        assert_eq!(queue.step_previous().unwrap(), Path::new("track1.flac"));
        assert!(queue.back_indices().is_empty());
        assert!(!queue.has_previous());

        // Start boundary reached again
        assert!(matches!(
            queue.step_previous(),
            Err(AudioError::InvalidState(_))
        ));

        assert_eq!(queue.played_indices(), vec![0, 1, 2, 1, 0]);
        assert_eq!(
            queue.played_history(),
            vec![
                PathBuf::from("track1.flac"),
                PathBuf::from("track2.flac"),
                PathBuf::from("track3.flac"),
                PathBuf::from("track2.flac"),
                PathBuf::from("track1.flac"),
            ]
        );
    }

    #[test]
    fn with_start_index_supports_preceding_fallback_and_validates_bounds() {
        assert!(matches!(
            TrackQueue::with_start_index(Vec::new(), 0),
            Err(AudioError::InvalidState(_))
        ));
        assert!(matches!(
            TrackQueue::with_start_index(sample_tracks(), 3),
            Err(AudioError::InvalidState(_))
        ));

        let mut queue = TrackQueue::with_start_index(sample_tracks(), 2).unwrap();
        assert_eq!(queue.current_index(), Some(2));
        assert!(queue.has_previous());
        assert!(!queue.has_next());

        assert_eq!(queue.step_previous().unwrap(), Path::new("track2.flac"));
        assert_eq!(queue.step_previous().unwrap(), Path::new("track1.flac"));
        assert!(matches!(
            queue.step_previous(),
            Err(AudioError::InvalidState(_))
        ));
    }

    #[test]
    fn inspects_consecutive_lookahead_and_remaining_tracks() {
        let mut queue = TrackQueue::new(sample_tracks());
        assert_eq!(queue.peek_next(), Some(Path::new("track2.flac")));
        assert_eq!(queue.peek_next_index(), Some(1));
        assert_eq!(queue.peek_previous(), None);
        assert_eq!(
            queue.remaining_tracks(),
            &[PathBuf::from("track2.flac"), PathBuf::from("track3.flac")]
        );
        assert!(queue.is_consecutive(0, 1));
        assert!(queue.is_consecutive(1, 2));
        assert!(!queue.is_consecutive(0, 2));
        assert!(!queue.is_consecutive(2, 3));

        queue.advance_next().unwrap();
        assert_eq!(queue.peek_next(), Some(Path::new("track3.flac")));
        assert_eq!(queue.peek_next_index(), Some(2));
        assert_eq!(queue.peek_previous(), Some(Path::new("track1.flac")));
        assert_eq!(queue.remaining_tracks(), &[PathBuf::from("track3.flac")]);

        queue.advance_next().unwrap();
        assert_eq!(queue.peek_next(), None);
        assert_eq!(queue.peek_next_index(), None);
        assert_eq!(queue.peek_previous(), Some(Path::new("track2.flac")));
        assert!(queue.remaining_tracks().is_empty());
    }
}
