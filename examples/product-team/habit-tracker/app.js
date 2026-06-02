// Habit Tracker — application entry point (seed skeleton).
//
// This file is intentionally a starting point. The product team builds
// it out toward .team/requirements.md: define a habit, mark it done for
// today, show the current streak, and persist locally across reloads.
//
// The open questions the Product Manager is still resolving (see requirements.md)
// shape what goes here — e.g. whether a streak breaks on the first
// missed day, or counts vs. a simple done/not-done.

const HABITS_KEY = "habit-tracker:habits";

function loadHabits() {
  // TODO(team): read the saved habits from localStorage under HABITS_KEY.
  return [];
}

function render() {
  // TODO(team): render the habit list, a per-habit "done today" control,
  // and the current streak into the #habits section.
  const habits = loadHabits();
  const root = document.getElementById("habits");
  if (root && habits.length === 0) {
    root.textContent = "No habits yet — the team builds the add-habit flow next.";
  }
}

document.addEventListener("DOMContentLoaded", render);
