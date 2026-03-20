// Vendure-like REST API benchmark server
// Simulates a minimal e-commerce backend: products, orders, auth
// Compiled with TypeScript AOT native compiler

// ── Data structures ──────────────────────────────────────────

class Product {
  id: number;
  name: string;
  slug: string;
  description: string;
  price: number;
  stock: number;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;

  constructor(id: number, name: string, price: number, stock: number) {
    this.id = id;
    this.name = name;
    this.slug = name.toLowerCase().replace(/ /g, "-");
    this.description = `Description for ${name}`;
    this.price = price;
    this.stock = stock;
    this.enabled = true;
    this.createdAt = new Date().toISOString();
    this.updatedAt = new Date().toISOString();
  }
}

class OrderLine {
  productId: number;
  productName: string;
  quantity: number;
  unitPrice: number;

  constructor(productId: number, productName: string, quantity: number, unitPrice: number) {
    this.productId = productId;
    this.productName = productName;
    this.quantity = quantity;
    this.unitPrice = unitPrice;
  }
}

class Order {
  id: number;
  code: string;
  state: string;
  customerId: number;
  lines: OrderLine[];
  totalPrice: number;
  createdAt: string;
  updatedAt: string;

  constructor(id: number, customerId: number) {
    this.id = id;
    this.code = "ORD-" + String(id).padStart(6, "0");
    this.state = "AddingItems";
    this.customerId = customerId;
    this.lines = [];
    this.totalPrice = 0;
    this.createdAt = new Date().toISOString();
    this.updatedAt = new Date().toISOString();
  }
}

class Customer {
  id: number;
  email: string;
  firstName: string;
  lastName: string;
  token: string;

  constructor(id: number, email: string, firstName: string, lastName: string) {
    this.id = id;
    this.email = email;
    this.firstName = firstName;
    this.lastName = lastName;
    this.token = "tok_" + String(id) + "_" + String(Math.floor(Math.random() * 1000000));
  }
}

// ── In-memory data store ─────────────────────────────────────

const products: Map<number, Product> = new Map();
const orders: Map<number, Order> = new Map();
const customers: Map<number, Customer> = new Map();
const tokenToCustomer: Map<string, number> = new Map();

let nextProductId = 1;
let nextOrderId = 1;
let nextCustomerId = 1;

// Seed with initial data
function seedData() {
  const sampleProducts = [
    ["Laptop Pro 15", 149999, 50],
    ["Wireless Mouse", 2999, 200],
    ["USB-C Hub", 4999, 150],
    ["Mechanical Keyboard", 12999, 75],
    ["4K Monitor", 49999, 30],
    ["Webcam HD", 7999, 100],
    ["Standing Desk", 89999, 20],
    ["Ergonomic Chair", 69999, 25],
    ["Headphones ANC", 29999, 80],
    ["Desk Lamp LED", 3999, 120],
  ];

  for (const item of sampleProducts) {
    const name = item[0] as string;
    const price = item[1] as number;
    const stock = item[2] as number;
    const p = new Product(nextProductId++, name, price, stock);
    products.set(p.id, p);
  }

  const c1 = new Customer(nextCustomerId++, "alice@example.com", "Alice", "Smith");
  const c2 = new Customer(nextCustomerId++, "bob@example.com", "Bob", "Jones");
  customers.set(c1.id, c1);
  customers.set(c2.id, c2);
  tokenToCustomer.set(c1.token, c1.id);
  tokenToCustomer.set(c2.token, c2.id);
}

// ── Auth helpers ─────────────────────────────────────────────

function getCustomerFromRequest(req: Request): Customer | null {
  const auth = req.headers.get("authorization") || "";
  if (!auth.startsWith("Bearer ")) return null;
  const token = auth.substring(7);
  const customerId = tokenToCustomer.get(token);
  if (customerId === undefined) return null;
  return customers.get(customerId) || null;
}

// ── Response helpers ─────────────────────────────────────────

function jsonResponse(data: object, status: number): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: new Headers({
      "Content-Type": "application/json",
      "X-Powered-By": "ts-aot-native",
    }),
  });
}

function errorResponse(message: string, status: number): Response {
  return jsonResponse({ error: message, status }, status);
}

// ── Route handlers ───────────────────────────────────────────

async function handleGetProducts(req: Request): Promise<Response> {
  const url = new URL(req.url);
  const page = parseInt(url.searchParams.get("page") || "1");
  const pageSize = parseInt(url.searchParams.get("pageSize") || "10");
  const search = url.searchParams.get("search") || "";

  let all: Product[] = [];
  products.forEach((p: Product) => {
    if (!search || p.name.toLowerCase().includes(search.toLowerCase())) {
      all.push(p);
    }
  });

  const total = all.length;
  const start = (page - 1) * pageSize;
  const items = all.slice(start, start + pageSize);

  return jsonResponse({
    items: items.map((p: Product) => ({
      id: p.id,
      name: p.name,
      slug: p.slug,
      price: p.price,
      stock: p.stock,
      enabled: p.enabled,
    })),
    totalItems: total,
    currentPage: page,
    totalPages: Math.ceil(total / pageSize),
  }, 200);
}

async function handleGetProduct(req: Request, id: number): Promise<Response> {
  const p = products.get(id);
  if (!p) return errorResponse("Product not found", 404);

  return jsonResponse({
    id: p.id,
    name: p.name,
    slug: p.slug,
    description: p.description,
    price: p.price,
    stock: p.stock,
    enabled: p.enabled,
    createdAt: p.createdAt,
    updatedAt: p.updatedAt,
  }, 200);
}

async function handleCreateProduct(req: Request): Promise<Response> {
  const body = await req.text();
  let data: { name?: string; price?: number; stock?: number };
  try {
    data = JSON.parse(body);
  } catch (e) {
    return errorResponse("Invalid JSON", 400);
  }

  if (!data.name || data.price === undefined) {
    return errorResponse("name and price are required", 400);
  }

  const p = new Product(nextProductId++, data.name, data.price, data.stock || 0);
  products.set(p.id, p);

  return jsonResponse({ id: p.id, name: p.name, price: p.price }, 201);
}

async function handleUpdateProduct(req: Request, id: number): Promise<Response> {
  const p = products.get(id);
  if (!p) return errorResponse("Product not found", 404);

  const body = await req.text();
  let data: { name?: string; price?: number; stock?: number; enabled?: boolean };
  try {
    data = JSON.parse(body);
  } catch (e) {
    return errorResponse("Invalid JSON", 400);
  }

  if (data.name !== undefined) p.name = data.name;
  if (data.price !== undefined) p.price = data.price;
  if (data.stock !== undefined) p.stock = data.stock;
  if (data.enabled !== undefined) p.enabled = data.enabled;
  p.updatedAt = new Date().toISOString();

  return jsonResponse({ id: p.id, name: p.name, price: p.price, stock: p.stock, enabled: p.enabled }, 200);
}

async function handleGetOrders(req: Request): Promise<Response> {
  const customer = getCustomerFromRequest(req);
  if (!customer) return errorResponse("Unauthorized", 401);

  let customerOrders: Order[] = [];
  orders.forEach((o: Order) => {
    if (o.customerId === customer.id) customerOrders.push(o);
  });

  return jsonResponse({
    items: customerOrders.map((o: Order) => ({
      id: o.id,
      code: o.code,
      state: o.state,
      totalPrice: o.totalPrice,
      lineCount: o.lines.length,
      createdAt: o.createdAt,
    })),
    totalItems: customerOrders.length,
  }, 200);
}

async function handleCreateOrder(req: Request): Promise<Response> {
  const customer = getCustomerFromRequest(req);
  if (!customer) return errorResponse("Unauthorized", 401);

  const o = new Order(nextOrderId++, customer.id);
  orders.set(o.id, o);

  return jsonResponse({ id: o.id, code: o.code, state: o.state }, 201);
}

async function handleAddToOrder(req: Request, orderId: number): Promise<Response> {
  const customer = getCustomerFromRequest(req);
  if (!customer) return errorResponse("Unauthorized", 401);

  const o = orders.get(orderId);
  if (!o) return errorResponse("Order not found", 404);
  if (o.customerId !== customer.id) return errorResponse("Forbidden", 403);

  const body = await req.text();
  let data: { productId: number; quantity: number };
  try {
    data = JSON.parse(body);
  } catch (e) {
    return errorResponse("Invalid JSON", 400);
  }

  const p = products.get(data.productId);
  if (!p) return errorResponse("Product not found", 404);
  if (p.stock < data.quantity) return errorResponse("Insufficient stock", 400);

  const line = new OrderLine(p.id, p.name, data.quantity, p.price);
  o.lines.push(line);
  let total = 0;
  for (const l of o.lines) {
    total = total + l.unitPrice * l.quantity;
  }
  o.totalPrice = total;
  o.updatedAt = new Date().toISOString();

  return jsonResponse({
    orderId: o.id,
    code: o.code,
    totalPrice: o.totalPrice,
    lines: o.lines.length,
  }, 200);
}

async function handleLogin(req: Request): Promise<Response> {
  const body = await req.text();
  let data: { email?: string };
  try {
    data = JSON.parse(body);
  } catch (e) {
    return errorResponse("Invalid JSON", 400);
  }

  let found: Customer | null = null;
  customers.forEach((c: Customer) => {
    if (c.email === data.email) found = c;
  });

  if (!found) return errorResponse("Invalid credentials", 401);
  const c = found as Customer;

  return jsonResponse({
    token: c.token,
    customerId: c.id,
    email: c.email,
    name: c.firstName + " " + c.lastName,
  }, 200);
}

async function handleHealth(_req: Request): Promise<Response> {
  return jsonResponse({
    status: "ok",
    products: products.size,
    orders: orders.size,
    customers: customers.size,
    uptime: process.pid,
  }, 200);
}

// ── Router ───────────────────────────────────────────────────

function extractId(pathname: string, prefix: string): number {
  const rest = pathname.substring(prefix.length);
  const seg = rest.split("/")[0];
  return parseInt(seg) || 0;
}

async function router(req: Request): Promise<Response> {
  const url = new URL(req.url);
  const pathname = url.pathname;
  const method = req.method;

  // Health
  if (method === "GET" && pathname === "/health") {
    return await handleHealth(req);
  }

  // Products
  if (method === "GET" && pathname === "/api/products") {
    return await handleGetProducts(req);
  }
  if (method === "POST" && pathname === "/api/products") {
    return await handleCreateProduct(req);
  }
  if (method === "GET" && pathname.startsWith("/api/products/")) {
    const id = extractId(pathname, "/api/products/");
    return await handleGetProduct(req, id);
  }
  if (method === "PUT" && pathname.startsWith("/api/products/")) {
    const id = extractId(pathname, "/api/products/");
    return await handleUpdateProduct(req, id);
  }

  // Orders
  if (method === "GET" && pathname === "/api/orders") {
    return await handleGetOrders(req);
  }
  if (method === "POST" && pathname === "/api/orders") {
    return await handleCreateOrder(req);
  }
  if (method === "POST" && pathname.startsWith("/api/orders/") && pathname.endsWith("/lines")) {
    const id = extractId(pathname, "/api/orders/");
    return await handleAddToOrder(req, id);
  }

  // Auth
  if (method === "POST" && pathname === "/api/auth/login") {
    return await handleLogin(req);
  }

  return errorResponse("Not Found", 404);
}

// ── Start server ─────────────────────────────────────────────

seedData();

const PORT = 19888;
serve(PORT, router);
