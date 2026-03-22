// Bun equivalent of vendure_bench.ts — in-memory e-commerce REST API

class Product {
  id: number; name: string; slug: string; description: string;
  price: number; stock: number; enabled: boolean;
  createdAt: string; updatedAt: string;
  constructor(id: number, name: string, price: number, stock: number) {
    this.id = id; this.name = name;
    this.slug = name.toLowerCase().replace(/ /g, '-');
    this.description = `Description for ${name}`;
    this.price = price; this.stock = stock; this.enabled = true;
    this.createdAt = new Date().toISOString();
    this.updatedAt = new Date().toISOString();
  }
}

class Order {
  id: number; customerId: number; lines: any[]; total: number;
  state: string; createdAt: string; updatedAt: string;
  constructor(id: number, customerId: number) {
    this.id = id; this.customerId = customerId; this.lines = [];
    this.total = 0; this.state = 'AddingItems';
    this.createdAt = new Date().toISOString();
    this.updatedAt = new Date().toISOString();
  }
}

class Customer {
  id: number; firstName: string; lastName: string; email: string;
  token: string; createdAt: string;
  constructor(id: number, firstName: string, lastName: string, email: string) {
    this.id = id; this.firstName = firstName; this.lastName = lastName;
    this.email = email;
    this.token = `tok_${id}_${Math.random().toString(36).substring(2, 10)}`;
    this.createdAt = new Date().toISOString();
  }
}

const products = new Map<number, Product>();
const orders = new Map<number, Order>();
const customers = new Map<number, Customer>();
const tokenToCustomer = new Map<string, number>();
let nextProductId = 1, nextOrderId = 1, nextCustomerId = 1;

function seedData() {
  const sampleProducts: [string, number, number][] = [
    ['Wireless Headphones', 9999, 50], ['USB-C Hub', 4999, 100],
    ['Mechanical Keyboard', 14999, 30], ['4K Webcam', 7999, 25],
    ['Standing Desk Mat', 3999, 75], ['Monitor Arm', 5999, 40],
    ['Laptop Stand', 2999, 60], ['Cable Management Kit', 1999, 200],
    ['Ergonomic Mouse', 8999, 45], ['Blue Light Glasses', 2499, 150],
  ];
  for (const [name, price, stock] of sampleProducts) {
    const id = nextProductId++;
    products.set(id, new Product(id, name, price, stock));
  }
  const sampleCustomers: [string, string, string][] = [
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

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } });
}

function extractId(pathname: string, prefix: string): number {
  const rest = pathname.substring(prefix.length);
  return parseInt(rest.split('/')[0]) || 0;
}

seedData();

const PORT = parseInt(process.env.PORT || '19890');

Bun.serve({
  port: PORT,
  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const { pathname } = url;
    const method = req.method;

    if (method === 'GET' && pathname === '/health') {
      return json({ status: 'ok', products: products.size, orders: orders.size, customers: customers.size });
    }
    if (method === 'GET' && pathname === '/api/products') {
      const page = parseInt(url.searchParams.get('page') || '1');
      const limit = parseInt(url.searchParams.get('limit') || '10');
      const skip = (page - 1) * limit;
      const all = [...products.values()].filter(p => p.enabled);
      return json({ items: all.slice(skip, skip + limit), total: all.length, page, limit });
    }
    if (method === 'GET' && pathname.startsWith('/api/products/')) {
      const id = extractId(pathname, '/api/products/');
      const p = products.get(id);
      if (!p) return json({ error: 'Not found' }, 404);
      return json(p);
    }
    if (method === 'GET' && pathname === '/api/orders') {
      const all = [...orders.values()];
      return json({ items: all, total: all.length });
    }
    if (method === 'POST' && pathname === '/api/orders') {
      const data = await req.json() as any;
      const id = nextOrderId++;
      const order = new Order(id, data.customerId || 0);
      orders.set(id, order);
      return json(order, 201);
    }
    if (method === 'POST' && pathname === '/api/auth/login') {
      const data = await req.json() as any;
      for (const c of customers.values()) {
        if (c.email === data.email) {
          return json({ token: c.token, customerId: c.id, email: c.email });
        }
      }
      return json({ error: 'Invalid credentials' }, 401);
    }
    return json({ error: 'Not Found' }, 404);
  },
});

console.log(`Bun server listening on :${PORT}`);
