let providersPromise = null;

function loadProviders() {
  providersPromise ??= fetch("/api/ai-providers", { cache: "no-store" }).then(
    (response) => {
      if (!response.ok) {
        throw new Error(response.statusText);
      }
      return response.json();
    },
  );
  return providersPromise;
}

function field(form, selector) {
  return form.querySelector(selector);
}

function setModelSuggestions(form, models) {
  const select = field(form, "[data-configuration-models]");
  const modelInput = field(form, "[data-configuration-model]");
  const placeholder = document.createElement("option");
  placeholder.value = "";
  placeholder.textContent = "Choose a suggested model";
  select.replaceChildren(
    placeholder,
    ...models.map((model) => {
      const option = document.createElement("option");
      option.value = model;
      option.textContent = model;
      return option;
    }),
  );
  select.value = modelInput.value;
}

function applyProvider(form, provider) {
  const nameInput = field(form, "[name=name]");
  if (!nameInput.value || nameInput.value === form.dataset.previousProviderName) {
    nameInput.value = provider.name;
  }
  form.dataset.previousProviderName = provider.name;
  field(form, "[data-configuration-method]").value = provider.adapter_kind;
  field(form, "[name=url]").value = provider.api_url;
  setModelSuggestions(form, provider.models);
}

document.addEventListener("change", async (event) => {
  const target = event.target;
  const form = target.closest?.("[data-configuration-form]");
  if (!form) {
    return;
  }
  if (target.matches("[data-configuration-models]")) {
    const modelInput = field(form, "[data-configuration-model]");
    if (target.value) {
      modelInput.value = target.value;
    }
    return;
  }
  if (
    !target.matches(
      "[data-configuration-provider], [data-configuration-method]",
    )
  ) {
    return;
  }

  let providers;
  try {
    providers = await loadProviders();
  } catch {
    return;
  }

  if (target.matches("[data-configuration-provider]")) {
    if (target.value === "custom") {
      const nameInput = field(form, "[name=name]");
      if (nameInput.value === form.dataset.previousProviderName) {
        nameInput.value = "";
      }
      delete form.dataset.previousProviderName;
      return;
    }
    const provider = providers.find((item) => item.name === target.value);
    if (provider) {
      applyProvider(form, provider);
    }
  } else if (target.matches("[data-configuration-method]")) {
    setModelSuggestions(
      form,
      providers
        .filter((provider) => provider.adapter_kind === target.value)
        .flatMap((provider) => provider.models),
    );
  }
});

document.addEventListener("input", (event) => {
  const target = event.target;
  const form = target.closest?.("[data-configuration-form]");
  if (!form || !target.matches("[data-configuration-model]")) {
    return;
  }
  field(form, "[data-configuration-models]").value = target.value;
});
