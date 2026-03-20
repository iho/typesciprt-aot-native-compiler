import 'reflect-metadata';
import { Module, Injectable, Controller, Get, Post, NotFoundException } from '@nestjs/common';
import { NestFactory } from '@nestjs/core';

// ── Service ───────────────────────────────────────────────────────────────────
@Injectable()
class AppService {
  private items: Map<string, string> = new Map();

  getHello(): string {
    return 'Hello from NestJS!';
  }

  getItem(id: string): string {
    const val = this.items.get(id);
    if (!val) throw new NotFoundException('Item ' + id + ' not found');
    return val;
  }

  createItem(id: string, value: string): string {
    this.items.set(id, value);
    return value;
  }
}

// ── Controller ────────────────────────────────────────────────────────────────
@Controller('api')
class AppController {
  constructor(private readonly appService: AppService) {}

  @Get()
  getRoot(): string {
    return this.appService.getHello();
  }

  @Get('hello')
  getHello(): string {
    return 'Hello World!';
  }

  @Post('echo')
  echo(): string {
    return 'echo!';
  }
}

// ── Module ─────────────────────────────────────────────────────────────────────
@Module({
  controllers: [AppController],
  providers: [AppService],
})
class AppModule {}

// ── Bootstrap ─────────────────────────────────────────────────────────────────
async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(AppModule);
  await app.listen(3000);
}

bootstrap();
