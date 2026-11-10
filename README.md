# MoveKernel ARM

Портирование MoveKernel на архитектуру AArch64 (ARM64).

- Загрузка в QEMU `virt` (Cortex-A72)
- Свой загрузчик (`boot.s`) и линкер-скрипт
- Драйвер virtio-gpu и вывод текста на framebuffer
- Свой шрифт 5x7

(пока через cargo objcopy и QEMU, позже добавим скрипты)

- Основной x86-репозиторий: [MoveKernel](https://github.com/krnMove/MoveKernel)
