// Test namespace import: import * as http from 'node:http'
import * as http from 'node:http';

const PORT = 19002;

const server = http.createServer((req: any, res: any) => {
  res.writeHead(200, { 'Content-Type': 'text/plain' });
  res.end('Hello from namespace import!');
});

server.listen(PORT);
