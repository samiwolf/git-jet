/* One-shot CDP probe: evaluates JS expressions against the running Git Jet
 * window over the WebView2 devtools protocol. */
const port = process.env.CDP_PORT || 9222;
const expressions = process.argv.slice(2);

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) {
  console.error("no page target");
  process.exit(1);
}

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = () => reject(new Error("websocket failed"));
});

let nextId = 0;
const pending = new Map();
ws.onmessage = (raw) => {
  const data = JSON.parse(raw.data);
  if (data.id && pending.has(data.id)) {
    pending.get(data.id)(data);
    pending.delete(data.id);
  }
};

function send(method, params = {}) {
  return new Promise((resolve) => {
    const id = ++nextId;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression) {
  const reply = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (reply.result?.exceptionDetails) {
    return { exception: reply.result.exceptionDetails.text };
  }
  return reply.result?.result?.value;
}

for (const expression of expressions) {
  console.log("---", expression.slice(0, 60), "---");
  console.log(JSON.stringify(await evaluate(expression), null, 2));
}
ws.close();
