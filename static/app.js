const notifications = document.getElementById("notifications");
const eventStream = new EventSource("/events");

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

const privacyPolicyUrls = {
  anthropic: "https://www.anthropic.com/legal/privacy",
  open_ai: "https://openai.com/policies/privacy-policy/",
  groq: "https://groq.com/privacy-policy/",
};
const providerNames = {
  anthropic: "Anthropic",
  open_ai: "OpenAI",
  groq: "Groq",
  open_ai_compatible: "Custom OpenAI-compatible provider",
};
const providerDefaultModels = {
  anthropic: "claude-sonnet-5-5",
  open_ai: "gpt-5.4-mini",
  groq: "",
  open_ai_compatible: "",
};
const providerDefaultBaseUrls = {
  anthropic: "https://api.anthropic.com",
  open_ai: "https://api.openai.com/v1",
  groq: "https://api.groq.com/openai/v1",
  open_ai_compatible: "",
};

function updateProviderPolicy() {
  const provider = document.getElementById("provider").value;
  const link = document.getElementById("privacy-policy");
  const policyUrl = privacyPolicyUrls[provider];
  link.hidden = !policyUrl;
  if (policyUrl) {
    link.href = policyUrl;
    link.textContent = `${providerNames[provider]} privacy policy`;
  }
  document.getElementById("privacy-policy-custom").textContent = policyUrl
    ? ""
    : " Consult the selected service's data-use and privacy terms before using AI features.";
  const baseUrlInput = document.getElementById("base-url");
  const custom = provider === "open_ai_compatible";
  baseUrlInput.disabled = !custom;
  baseUrlInput.value = custom
    ? baseUrlInput.value
    : providerDefaultBaseUrls[provider];
  document.getElementById("base-url-label").hidden = !custom;
  baseUrlInput.hidden = !custom;
  document.getElementById("base-url-help").hidden = !custom;
}

async function loadSettings() {
  const status = document.getElementById("settings-status");
  try {
    const response = await fetch("/api/settings", { cache: "no-store" });
    if (!response.ok) {
      throw new Error(await response.text());
    }
    const settings = await response.json();
    document.getElementById("provider").value = settings.provider;
    document.getElementById("model-id").value = settings.model_id;
    document.getElementById("base-url").value = settings.base_url;
    document.getElementById("api-key-status").textContent =
      settings.api_key_saved ? "An API key is saved in the operating system credential vault." : "No API key is saved for this provider.";
    updateProviderPolicy();
  } catch (error) {
    status.textContent = `Could not load AI settings: ${error.message}`;
  }
}

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

document.getElementById("provider").addEventListener("change", () => {
  document.getElementById("model-id").value =
    providerDefaultModels[document.getElementById("provider").value];
  document.getElementById("base-url").value =
    providerDefaultBaseUrls[document.getElementById("provider").value];
  document.getElementById("settings-status").textContent =
    "Model ID changed to this provider's suggested starting model. You can edit it if needed.";
  updateProviderPolicy();
});

document.getElementById("settings-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const status = document.getElementById("settings-status");
  const provider = document.getElementById("provider").value;
  const payload = {
    provider,
    model_id: document.getElementById("model-id").value,
    base_url: document.getElementById("base-url").value,
    api_key: document.getElementById("api-key").value,
  };
  try {
    const response = await fetch("/api/settings", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (!response.ok) {
      throw new Error(await response.text());
    }
    const settings = await response.json();
    document.getElementById("api-key").value = "";
    document.getElementById("api-key-status").textContent =
      settings.api_key_saved ? "An API key is saved in the operating system credential vault." : "No API key is saved for this provider.";
    status.textContent = "Settings saved.";
  } catch (error) {
    status.textContent = `Could not save settings: ${error.message}`;
  }
});

document.getElementById("forget-api-key").addEventListener("click", async () => {
  const status = document.getElementById("settings-status");
  try {
    const response = await fetch("/api/settings", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        provider: document.getElementById("provider").value,
        model_id: document.getElementById("model-id").value,
        base_url: document.getElementById("base-url").value,
        forget_api_key: true,
      }),
    });
    if (!response.ok) {
      throw new Error(await response.text());
    }
    document.getElementById("api-key").value = "";
    document.getElementById("api-key-status").textContent = "No API key is saved for this provider.";
    status.textContent = "Saved API key forgotten.";
  } catch (error) {
    status.textContent = `Could not forget API key: ${error.message}`;
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

loadSettings();

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
