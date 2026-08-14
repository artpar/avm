bits 16
org 0x7c00

start:
    cli
    xor ax, ax
    mov ds, ax
    mov ss, ax
    mov sp, 0x7c00
    sti

    mov ax, 0x0013
    int 0x10
    mov byte [color], 1
    call paint
    mov si, ready_message
    mov dx, 0x00e9

signal_ready:
    lodsb
    test al, al
    jz wait_for_key
    out dx, al
    jmp signal_ready

wait_for_key:
    xor ah, ah
    int 0x16
    inc byte [color]
    call paint
    mov dx, 0x00e9
    mov al, 'K'
    out dx, al
    jmp wait_for_key

paint:
    mov ax, 0xa000
    mov es, ax
    xor di, di
    mov al, [color]
    mov ah, al
    mov cx, 32000
    rep stosw
    ret

color: db 1
ready_message: db 'READY', 10, 0

times 510 - ($ - $$) db 0
dw 0xaa55
