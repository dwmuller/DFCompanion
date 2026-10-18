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

document.body.addEventListener("htmx:afterRequest", (event) => {
  if (event.detail.successful) {
    document.getElementById("message").value = "";
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
