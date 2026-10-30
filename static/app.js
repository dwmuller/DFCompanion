const notifications = document.getElementById("notifications");
const eventStream = new EventSource("/events");

// The server replays the recorded conversation on every (re)connection.
eventStream.onopen = () => notifications.replaceChildren();

// Companion and user text is untrusted Markdown (model output, game names), so
// it is always sanitized, links open safely, and images are dropped (the page's
// CSP blocks them too). Everything else stays plain text.
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A" && node.hasAttribute("href")) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

function renderMarkdown(markdown) {
  return DOMPurify.sanitize(marked.parse(markdown, { breaks: true, gfm: true }), {
    FORBID_TAGS: ["img", "style"],
    FORBID_ATTR: ["style"],
  });
}

function addMarkdownItem(label, markdown) {
  const item = document.createElement("li");
  item.className = "markdown";
  const name = document.createElement("strong");
  name.textContent = `${label} `;
  const body = document.createElement("div");
  body.className = "markdown-body";
  body.innerHTML = renderMarkdown(markdown);
  item.append(name, body);
  notifications.appendChild(item);
  notifications.scrollTop = notifications.scrollHeight;
}

eventStream.onmessage = (event) => {
  const speech = /^(You|Companion): ([\s\S]*)$/.exec(event.data);
  if (speech) {
    addMarkdownItem(`${speech[1]}:`, speech[2]);
    return;
  }
  const item = document.createElement("li");
  item.textContent = event.data;
  notifications.appendChild(item);
  notifications.scrollTop = notifications.scrollHeight;
};

eventStream.addEventListener("game_event", (event) => {
  const item = document.createElement("li");
  item.className = "game-event";
  item.textContent = `Game event: ${event.data}`;
  notifications.appendChild(item);
  notifications.scrollTop = notifications.scrollHeight;
});

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

eventStream.addEventListener("unsolicited", (event) => {
  addMarkdownItem("Companion:", event.data);
  // The server replays history right after (re)connecting; only beep for live comments.
  if (Date.now() - connectedAt > 2000) {
    beep();
  }
});

let connectedAt = Date.now();
eventStream.addEventListener("open", () => (connectedAt = Date.now()));

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

eventStream.addEventListener("warning", (event) => {
  const item = document.createElement("li");
  item.textContent = event.data;
  notifications.appendChild(item);
});

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
