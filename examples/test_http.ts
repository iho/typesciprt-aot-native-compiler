import * as http from 'node:http';

const server = http.createServer((req: any, res: any) => {
  res.writeHead(200, { 'Content-Type': 'text/plain' });
  res.end('Hello from node:http!');
});

server.listen(19091);
