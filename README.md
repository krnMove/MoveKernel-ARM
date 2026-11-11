# MoveKernel ARM

Портирование MoveKernel на архитектуру AArch64 (ARM64).

- Полный список изменений: [CHANGELOG.md](CHANGELOG.md)
- Загрузка в QEMU `virt` (Cortex-A57)
- Свой загрузчик (`boot.s`) и линкер-скрипт
- Драйвер virtio-gpu и вывод текста на framebuffer
- GICv3 (Generic Timer отключён)
- Кооперативный планировщик задач без таймерных прерываний
- Ввод с virtio-клавиатуры и интерактивная консоль
- ARM MMU: identity-map для RAM, GIC и virtio MMIO
- 1 MiB heap с выравниванием, освобождением и повторным использованием блоков
- Virtio-blk и чтение/запись FAT32 (каталог корня, имена 8.3)
- Свой шрифт 5x7

## Запуск в QEMU

Соберите ядро и тестовый FAT32-образ с файлом `HELLO.TXT` одной командой:

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-arm.ps1
```

Запустите собранную систему в QEMU:

```powershell
powershell -ExecutionPolicy Bypass -File tools\run-arm.ps1
```

По умолчанию образ создаётся в `target\fat32-test.img`. Скрипт образа
использует безопасный режим создания и не перезаписывает существующий файл.
Чтобы собрать образ в другом месте, укажите, например:

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-arm.ps1 `
  -ImagePath target\my-test.img
powershell -ExecutionPolicy Bypass -File tools\run-arm.ps1 `
  -ImagePath target\my-test.img
```

При старте ядро включает MMU и проверяет, что allocator может выделять,
выравнивать, освобождать и повторно использовать память. Планировщик запускает
две кооперативные фоновые задачи; периодический Generic Timer отключён.
Кликните по окну QEMU и вводите команды в консоль: `/HELP`, `/TASKS`, `/LS`,
`/CAT HELLO.TXT`, `/WRITE NOTE.TXT текст`, `/CLEAR`. Список файлов показывает
корень `MOVEKERNEL/` и записи корневого каталога. `/WRITE` создаёт или
перезаписывает файл в корневом каталоге FAT32; имена поддерживаются в формате
8.3. Запись текста ограничена длиной команды оболочки. Если диск не подключён
или образ не распознан, сообщение об ошибке появится при запуске, а остальные
команды останутся доступны.

Минимальные проверки перед запуском:

```powershell
cargo fmt -- --check
cargo check --target aarch64-unknown-none --release
```

- Основной x86-репозиторий: [MoveKernel](https://github.com/krnMove/MoveKernel)
