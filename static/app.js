const notifications = document.getElementById("notifications");
// Game events are hidden by default; the choice is remembered.
const showGameEvents = document.getElementById("show-game-events");
function applyGameEventVisibility() {
  notifications.classList.toggle("hide-game-events", !showGameEvents.checked);
  notifications.scrollTop = notifications.scrollHeight;
}
showGameEvents.checked = localStorage.getItem("showGameEvents") === "true";
showGameEvents.addEventListener("change", () => {
  localStorage.setItem("showGameEvents", showGameEvents.checked);
  applyGameEventVisibility();
});
applyGameEventVisibility();

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

// Timed turns cost tokens, so they are opt-in. The browser remembers the
// choice and tells the service on each page load.
const timedTurns = document.getElementById("timed-turns");
async function sendTimedTurns() {
  localStorage.setItem("timedTurns", timedTurns.checked);
  await fetch("/api/timed-turns", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ enabled: timedTurns.checked }),
  });
}
timedTurns.checked = localStorage.getItem("timedTurns") === "true";
timedTurns.addEventListener("change", sendTimedTurns);
sendTimedTurns();

document.getElementById("chat-form").addEventListener("htmx:after:request", (event) => {
  if (event.detail.ctx.response.raw.ok) {
    document.getElementById("message").value = "";
  }
});

document.getElementById("message").addEventListener("keydown", (event) => {
  // Enter sends; Shift+Enter inserts a newline. Ignore Enter while composing (IME).
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    if (event.currentTarget.value.trim() !== "") {
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
