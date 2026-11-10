# MoveKernel ARM

Портирование MoveKernel на архитектуру AArch64 (ARM64).

- Загрузка в QEMU `virt` (Cortex-A57)
- Свой загрузчик (`boot.s`) и линкер-скрипт
- Драйвер virtio-gpu и вывод текста на framebuffer
- GICv3 и прерывания ARM Generic Timer
- ARM MMU: identity-map для RAM, GIC и virtio MMIO
- 1 MiB heap с выравниванием, освобождением и повторным использованием блоков
- Virtio-blk и чтение FAT32 (каталог корня, имена 8.3)
- Свой шрифт 5x7

## Запуск в QEMU

Соберите ядро и тестовый FAT32-образ с файлом `HELLO.TXT`:

```powershell
cargo build --target aarch64-unknown-none --release
powershell -ExecutionPolicy Bypass -File tools\create-fat32-image.ps1 `
  -OutputPath target\fat32-test.img
```

Запустите QEMU с GICv3, графическим адаптером и virtio-диском:

```powershell
qemu-system-aarch64 `
  -machine virt,gic-version=3 `
  -cpu cortex-a57 `
  -m 128M `
  -kernel target\aarch64-unknown-none\release\movekernel-arm `
  -device virtio-gpu-device `
  -drive if=none,id=disk0,file=target\fat32-test.img,format=raw `
  -device virtio-blk-device,drive=disk0 `
  -global virtio-mmio.force-legacy=false `
  -display sdl `
  -serial none
```

При старте ядро включает MMU и проверяет, что allocator может выделять,
выравнивать, освобождать и повторно использовать память. Консоль показывает
результат проверки, тики таймера, содержимое корневого каталога и текст из
`HELLO.TXT`. Драйвер FAT32 работает только на чтение; для имён файлов сейчас
поддерживается формат 8.3. Если диск не подключён или образ не распознан,
ошибка выводится на экран, а консоль продолжает работать.

Минимальные проверки перед запуском:

```powershell
cargo fmt -- --check
cargo check --target aarch64-unknown-none --release
```

- Основной x86-репозиторий: [MoveKernel](https://github.com/krnMove/MoveKernel)
