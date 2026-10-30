const notifications = document.getElementById("notifications");
// Hidden notification kinds are filtered by CSS from these checkboxes; the
// choices are remembered. Both kinds are hidden by default.
for (const id of ["show-game-events", "show-ai-usage"]) {
  const checkbox = document.getElementById(id);
  checkbox.checked = localStorage.getItem(id) === "true";
  checkbox.addEventListener("change", () => {
    localStorage.setItem(id, checkbox.checked);
    notifications.scrollTop = notifications.scrollHeight;
  });
}

// The server sends a "beep" event only for live (not replayed) companion comments.
document.addEventListener("beep", beep);

function beep() {
  try {
    const context = new AudioContext();
    const oscillator = context.createOscillator();
    const gain = context.createGain();
    oscillator.frequency.value = 660;
    gain.gain.value = 0.05;
    oscillator.connect(gain).connect(context.destination);
    oscillator.start();
    oscillator.stop(context.currentTime + 0.15);
    oscillator.onended = () => context.close();
  } catch {
    // Audio may be blocked until the page has had user interaction.
  }
}

// Automatic turns cost tokens, so they are opt-in. The browser remembers the
// choices; the form's hx-trigger="load" then tells the service on each page load.
for (const id of ["event-turns", "periodic-turns"]) {
  const checkbox = document.getElementById(id);
  checkbox.checked = localStorage.getItem(id) === "true";
  checkbox.addEventListener("change", () => localStorage.setItem(id, checkbox.checked));
}

// Sending clears the box through an out-of-band swap in the server's reply.
// Enter-to-send stays here: htmx trigger filters need eval, which the CSP forbids.
// Delegated, because a successful send replaces the textarea element.
document.addEventListener("keydown", (event) => {
  // Enter sends; Shift+Enter inserts a newline. Ignore Enter while composing (IME).
  if (
    event.target.id === "message" &&
    event.key === "Enter" &&
    !event.shiftKey &&
    !event.isComposing
  ) {
    event.preventDefault();
    if (event.target.value.trim() !== "") {
      document.getElementById("chat-form").requestSubmit();
    }
  }
});

document.getElementById("copy-dfhack-command").addEventListener("click", async () => {
  const command = document.getElementById("dfhack-command").textContent;
  const status = document.getElementById("copy-status");
  try {
    await navigator.clipboard.writeText(command);
    status.textContent = "Copied.";
  } catch (error) {
    status.textContent = `Could not copy command: ${error.message}`;
  }
});
