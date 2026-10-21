const notifications = document.getElementById("notifications");
const eventStream = new EventSource("/events");
const interactionMethodLabels = {
  OpenAI: "OpenAI Chat Completions",
  OpenAIResp: "OpenAI Responses",
  Anthropic: "Anthropic",
};

eventStream.onmessage = (event) => {
  const item = document.createElement("li");
  item.textContent = event.data;
  notifications.appendChild(item);
  notifications.scrollTop = notifications.scrollHeight;
};

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
