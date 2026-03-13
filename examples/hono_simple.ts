// Simple Hono-like HTTP server using supported features.
// Routes: GET /  GET /hello/:name  POST /echo  GET /url-test

class HTTPException extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

// Simple router
const routes: { method: string; path: string; handler: (req: Request) => Promise<Response> }[] = [];

function get(path: string, handler: (req: Request) => Promise<Response>) {
  routes.push({ method: "GET", path, handler });
}

function post(path: string, handler: (req: Request) => Promise<Response>) {
  routes.push({ method: "POST", path, handler });
}

function matchRoute(method: string, pathname: string): ((req: Request) => Promise<Response>) | null {
  for (const route of routes) {
    if (route.method !== method) continue;
    // Exact match
    if (route.path === pathname) return route.handler;
    // Pattern match: /hello/:name
    const routeParts = route.path.split("/");
    const pathParts = pathname.split("/");
    if (routeParts.length !== pathParts.length) continue;
    let match = true;
    for (let i = 0; i < routeParts.length; i++) {
      if (!routeParts[i].startsWith(":") && routeParts[i] !== pathParts[i]) {
        match = false;
        break;
      }
    }
    if (match) return route.handler;
  }
  return null;
}

function getParam(path: string, pattern: string, name: string): string {
  const routeParts = pattern.split("/");
  const pathParts = path.split("/");
  for (let i = 0; i < routeParts.length; i++) {
    if (routeParts[i] === ":" + name) {
      return pathParts[i] || "";
    }
  }
  return "";
}

// Route definitions
get("/", async (req: Request) => {
  return new Response("Hello from native Hono!", { status: 200 });
});

get("/hello/:name", async (req: Request) => {
  const url = new URL(req.url);
  const parts = url.pathname.split("/");
  const name = parts[2] || "world";
  const greeting = `Hello, ${name}! You requested: ${url.pathname}`;
  return new Response(greeting, { status: 200 });
});

get("/url-test", async (req: Request) => {
  const url = new URL(req.url);
  const search = url.search;
  const pathname = url.pathname;
  const origin = url.origin;
  const result = JSON.stringify({
    pathname,
    search,
    origin,
    query_foo: url.searchParams.get("foo") || "not set",
  });
  return new Response(result, { status: 200 });
});

post("/echo", async (req: Request) => {
  const body = await req.text();
  return new Response(`Echo: ${body}`, { status: 200 });
});

get("/headers", async (req: Request) => {
  const accept = req.headers.get("accept") || "none";
  return new Response(`Accept: ${accept}`, { status: 200 });
});

// Main dispatch
serve(8888, async (req: Request) => {
  const url = new URL(req.url);
  const pathname = url.pathname;
  const method = req.method || "GET";

  const handler = matchRoute(method, pathname);
  if (handler !== null) {
    try {
      return await handler(req);
    } catch (e) {
      const msg = (e as HTTPException).message || "Internal error";
      return new Response(`Error: ${msg}`, { status: 500 });
    }
  }

  return new Response("Not Found", { status: 404 });
});
