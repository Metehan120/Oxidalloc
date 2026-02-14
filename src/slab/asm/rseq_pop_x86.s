.intel_syntax noprefix
.section .text

# Sign as Global Assembly point
.global rseq_pop_header
# Sign type as function
.type rseq_pop_header, @function

rseq_pop_header:
    # Register rseq_cs_descriptor (Critical Section Descriptor)
    lea rax, [rip + rseq_cs_descriptor]
    mov [rsi], rax

# This is steal safe because if CPU pipeline changes kernel will restart the sequence
.start:
    mov rax, [rdi]
    # Check if null
    test rax, rax
    # Exit if null
    jz .post

    # Pop the header and return
    mov rdx, [rax]

    # Decrement the reference count
    mov r8, [rcx]
    dec r8

    mov [rdi], rdx
    mov [rcx], r8

# Signature for kernel to handle
.long 0x53053053
.abort:
    # Clear rseq_cs_descriptor and restart
    mov qword ptr [rsi], 0
    jmp rseq_pop_header

.post:
    # Clear rseq_cs_descriptor and exit
    mov qword ptr [rsi], 0
    ret

.section data
.align 32
rseq_cs_descriptor:
    .long 0, 0
    .quad .start
    .quad .post
    .quad .abort
