// node:http test server using named imports.
// Port is set via a placeholder replaced by the test harness.
import { createServer } from 'node:http';

const PORT = 19001;

const server = createServer((req: any, res: any) => {
  const method: string = req.method || 'GET';
  const url: string = req.url || '/';

  if (method === 'GET' && url === '/') {
    res.writeHead(200, { 'Content-Type': 'text/plain' });
    res.end('Hello from node:http!');
    return;
  }

  if (method === 'GET' && url === '/hello') {
    res.writeHead(200, { 'Content-Type': 'text/plain' });
    res.end('Hello World');
    return;
  }

  if (method === 'POST' && url === '/echo') {
    res.writeHead(200, { 'Content-Type': 'text/plain' });
    res.end('Echo: ' + req.body);
    return;
  }

  res.writeHead(404, { 'Content-Type': 'text/plain' });
  res.end('Not Found');
});

server.listen(PORT);
