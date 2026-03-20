// Real NestJS integration — uses actual @nestjs/common decorators from the git submodule
// and our native bootstrap (which replaces @nestjs/core).

import { Module, Injectable, Controller, Get, Post } from '../nest/packages/common/decorators/index'
import { bootstrapReal } from './nest-native-real'

// ── Services ──────────────────────────────────────────────────────────────────

@Injectable()
class AppService {
  getHello(): string {
    return 'Hello from real NestJS!'
  }

  getWorld(): string {
    return 'Hello World!'
  }
}

// ── Controllers ───────────────────────────────────────────────────────────────

@Controller('/')
class AppController {
  private appService: AppService

  constructor() {
    this.appService = new AppService()
  }

  @Get('/')
  getHello(): string {
    return this.appService.getHello()
  }

  @Get('/world')
  getWorld(): string {
    return this.appService.getWorld()
  }

  @Post('/echo')
  postEcho(): string {
    return 'echo!'
  }
}

// ── Module ────────────────────────────────────────────────────────────────────

@Module({
  controllers: [AppController],
  providers:   [AppService],
})
class AppModule {}

// ── Bootstrap ─────────────────────────────────────────────────────────────────

bootstrapReal(AppModule, 13001)
