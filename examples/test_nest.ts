import { Module, Injectable, Controller, Get, Post } from './nest-decorators'
import { bootstrapNative } from './nest-native'

// ── Services ─────────────────────────────────────────────────────────────────

@Injectable()
class AppService {
  getHello(): string {
    return 'Hello from NestJS!'
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

bootstrapNative(AppModule, 3000)
