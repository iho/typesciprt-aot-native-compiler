import { Client } from './pg_client';

async function main() {
  console.log('start');
  const c = new Client({ host: 'localhost', port: 5432, user: 'bench', database: 'bench' });
  console.log('created');
  await c.connect();
  console.log('connected!');
  c.end();
}
main();
