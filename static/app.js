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
