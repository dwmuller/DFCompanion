const notifications = document.getElementById("notifications");
const eventStream = new EventSource("/events");
const interactionMethodLabels = {
  OpenAI: "OpenAI Chat Completions",
  OpenAIResp: "OpenAI Responses",
  Anthropic: "Anthropic",
};

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

async function loadConfigurations() {
  const selector = document.getElementById("active-configuration");
  const status = document.getElementById("configuration-status");
  try {
    const response = await fetch("/api/ai-configurations", { cache: "no-store" });
    if (!response.ok) {
      throw new Error(await response.text());
    }
    const result = await response.json();
    selector.replaceChildren();
    if (result.configurations.length === 0) {
      selector.add(new Option("No AI configurations", ""));
      status.textContent = "Create an AI configuration before starting a chat.";
      return;
    }
    selector.add(new Option("Choose a configuration", ""));
    for (const configuration of result.configurations) {
      const keyStatus = configuration.api_key_saved ? "" : " (API key needed)";
      selector.add(new Option(
        `${configuration.name} — ${interactionMethodLabels[configuration.interaction_method] ?? configuration.interaction_method}${keyStatus}`,
        configuration.id,
      ));
    }
    selector.value = result.active_id ?? "";
    status.textContent = result.active_id
      ? "Chat messages and collected reports will use the selected configuration."
      : "Select an AI configuration to enable chat.";
  } catch (error) {
    status.textContent = `Could not load AI configurations: ${error.message}`;
  }
}

document.getElementById("active-configuration").addEventListener("change", async (event) => {
  const status = document.getElementById("configuration-status");
  try {
    const response = await fetch("/api/ai-configurations/active", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: event.currentTarget.value || null }),
    });
    if (!response.ok) {
      throw new Error(await response.text());
    }
    await loadConfigurations();
  } catch (error) {
    status.textContent = `Could not select configuration: ${error.message}`;
    await loadConfigurations();
  }
});

document.getElementById("chat-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const status = document.getElementById("chat-status");
  try {
    const response = await fetch("/chat", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams(new FormData(form)),
    });
    const result = await response.text();
    if (!response.ok) {
      throw new Error(result);
    }
    status.textContent = result;
    document.getElementById("message").value = "";
  } catch (error) {
    status.textContent = `Could not send message: ${error.message}`;
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

document.getElementById("retry-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const status = document.getElementById("retry-status");
  try {
    const response = await fetch("/retry", { method: "POST" });
    if (!response.ok) {
      throw new Error(await response.text());
    }
    status.textContent = await response.text();
  } catch (error) {
    status.textContent = `Could not retry: ${error.message}`;
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

loadConfigurations();
