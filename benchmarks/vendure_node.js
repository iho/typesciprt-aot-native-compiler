// Node.js equivalent of vendure_bench.ts
// Used for benchmarking the TypeScript AOT native compiler vs Node.js

const http = require('http');

// ── Data structures ──────────────────────────────────────────

class Product {
  constructor(id, name, price, stock) {
    this.id = id;
    this.name = name;
    this.slug = name.toLowerCase().replace(/ /g, '-');
    this.description = `Description for ${name}`;
    this.price = price;
    this.stock = stock;
    this.enabled = true;
    this.createdAt = new Date().toISOString();
    this.updatedAt = new Date().toISOString();
  }
}

class Order {
  constructor(id, customerId) {
    this.id = id;
    this.customerId = customerId;
    this.lines = [];
    this.total = 0;
    this.state = 'AddingItems';
    this.createdAt = new Date().toISOString();
    this.updatedAt = new Date().toISOString();
  }
}

class OrderLine {
  constructor(productId, productName, quantity, unitPrice) {
    this.productId = productId;
    this.productName = productName;
    this.quantity = quantity;
    this.unitPrice = unitPrice;
    this.linePrice = quantity * unitPrice;
  }
}

class Customer {
  constructor(id, firstName, lastName, email) {
    this.id = id;
    this.firstName = firstName;
    this.lastName = lastName;
    this.email = email;
    this.token = `tok_${id}_${Math.random().toString(36).substring(2, 10)}`;
    this.createdAt = new Date().toISOString();
  }
}

// ── In-memory store ──────────────────────────────────────────

const products = new Map();
const orders = new Map();
const customers = new Map();
const tokenToCustomer = new Map();

let nextProductId = 1;
let nextOrderId = 1;
let nextCustomerId = 1;

function seedData() {
  const sampleProducts = [
    ['Wireless Headphones', 9999, 50],
    ['USB-C Hub', 4999, 100],
    ['Mechanical Keyboard', 14999, 30],
    ['4K Webcam', 7999, 25],
    ['Standing Desk Mat', 3999, 75],
    ['Monitor Arm', 5999, 40],
    ['Laptop Stand', 2999, 60],
    ['Cable Management Kit', 1999, 200],
    ['Ergonomic Mouse', 8999, 45],
    ['Blue Light Glasses', 2499, 150],
  ];

  for (const [name, price, stock] of sampleProducts) {
    const id = nextProductId++;
    products.set(id, new Product(id, name, price, stock));
  }

  const sampleCustomers = [
    ['Alice', 'Smith', 'alice@example.com'],
    ['Bob', 'Jones', 'bob@example.com'],
    ['Carol', 'White', 'carol@example.com'],
  ];

  for (const [firstName, lastName, email] of sampleCustomers) {
    const id = nextCustomerId++;
    const c = new Customer(id, firstName, lastName, email);
    customers.set(id, c);
    tokenToCustomer.set(c.token, id);
  }
}

// ── Helpers ──────────────────────────────────────────────────

function jsonResponse(body, status = 200) {
  return { body: JSON.stringify(body), status, headers: { 'Content-Type': 'application/json' } };
}

function errorResponse(message, status = 400) {
  return jsonResponse({ error: message }, status);
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let data = '';
    req.on('data', chunk => { data += chunk; });
    req.on('end', () => {
      try { resolve(data ? JSON.parse(data) : {}); }
      catch (e) { resolve({}); }
    });
    req.on('error', reject);
  });
}

// ── Handlers ─────────────────────────────────────────────────

async function handleGetProducts(req, url) {
  const page = parseInt(url.searchParams.get('page') || '1');
  const limit = parseInt(url.searchParams.get('limit') || '10');
  const skip = (page - 1) * limit;

  const all = [...products.values()].filter(p => p.enabled);
  const items = all.slice(skip, skip + limit);

  return jsonResponse({ items, total: all.length, page, limit });
}

async function handleGetProduct(req, id) {
  const product = products.get(id);
  if (!product) return errorResponse('Product not found', 404);
  return jsonResponse(product);
}

async function handleCreateProduct(req) {
  const data = await readBody(req);
  if (!data.name || !data.price) return errorResponse('name and price required');
  const id = nextProductId++;
  const product = new Product(id, data.name, data.price, data.stock || 0);
  products.set(id, product);
  return jsonResponse(product, 201);
}

async function handleUpdateProduct(req, id) {
  const product = products.get(id);
  if (!product) return errorResponse('Product not found', 404);
  const data = await readBody(req);
  if (data.name !== undefined) product.name = data.name;
  if (data.price !== undefined) product.price = data.price;
  if (data.stock !== undefined) product.stock = data.stock;
  if (data.enabled !== undefined) product.enabled = data.enabled;
  product.updatedAt = new Date().toISOString();
  return jsonResponse(product);
}

async function handleGetOrders(req) {
  const all = [...orders.values()];
  return jsonResponse({ items: all, total: all.length });
}

async function handleCreateOrder(req) {
  const data = await readBody(req);
  const customerId = data.customerId || 0;
  const id = nextOrderId++;
  const order = new Order(id, customerId);
  orders.set(id, order);
  return jsonResponse(order, 201);
}

async function handleAddToOrder(req, orderId) {
  const order = orders.get(orderId);
  if (!order) return errorResponse('Order not found', 404);
  const data = await readBody(req);
  const productId = data.productId || 0;
  const quantity = data.quantity || 1;
  const product = products.get(productId);
  if (!product) return errorResponse('Product not found', 404);
  const line = new OrderLine(productId, product.name, quantity, product.price);
  order.lines.push(line);
  order.total += line.linePrice;
  order.updatedAt = new Date().toISOString();
  return jsonResponse(order);
}

async function handleLogin(req) {
  const data = await readBody(req);
  if (!data.email) return errorResponse('email required');

  let found = null;
  for (const c of customers.values()) {
    if (c.email === data.email) { found = c; break; }
  }

  if (!found) return errorResponse('Invalid credentials', 401);
  return jsonResponse({
    token: found.token,
    customerId: found.id,
    email: found.email,
    name: found.firstName + ' ' + found.lastName,
  });
}

async function handleHealth() {
  return jsonResponse({
    status: 'ok',
    products: products.size,
    orders: orders.size,
    customers: customers.size,
    uptime: process.pid,
  });
}

function extractId(pathname, prefix) {
  const rest = pathname.substring(prefix.length);
  const seg = rest.split('/')[0];
  return parseInt(seg) || 0;
}

async function router(req, url) {
  const { pathname } = url;
  const { method } = req;

  if (method === 'GET' && pathname === '/health') return handleHealth();
  if (method === 'GET' && pathname === '/api/products') return handleGetProducts(req, url);
  if (method === 'POST' && pathname === '/api/products') return handleCreateProduct(req);
  if (method === 'GET' && pathname.startsWith('/api/products/')) {
    return handleGetProduct(req, extractId(pathname, '/api/products/'));
  }
  if (method === 'PUT' && pathname.startsWith('/api/products/')) {
    return handleUpdateProduct(req, extractId(pathname, '/api/products/'));
  }
  if (method === 'GET' && pathname === '/api/orders') return handleGetOrders(req);
  if (method === 'POST' && pathname === '/api/orders') return handleCreateOrder(req);
  if (method === 'POST' && pathname.startsWith('/api/orders/') && pathname.endsWith('/lines')) {
    return handleAddToOrder(req, extractId(pathname, '/api/orders/'));
  }
  if (method === 'POST' && pathname === '/api/auth/login') return handleLogin(req);

  return errorResponse('Not Found', 404);
}

seedData();

const PORT = parseInt(process.env.PORT || '19889');

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  try {
    const response = await router(req, url);
    res.writeHead(response.status, response.headers);
    res.end(response.body);
  } catch (err) {
    res.writeHead(500, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: String(err) }));
  }
});

server.listen(PORT, () => {
  console.log(`Node.js vendure server listening on :${PORT}`);
});
