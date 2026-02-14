.intel_syntax noprefix
.section .text

.global rseq_push_header
.type rseq_push_header, @function

rseq_push_header:
    # Regsiter rseq_cs_descriptor (Critical Section Descriptor)
    lea rax, [rip + rseq_push_descriptor]
    mov [rdx], rax

.push_start:
    # Prepare: Set new_node->next = current_head
    mov rax, [rdi]
    mov [rsi], rax

    mov r8, [rcx]
    inc r8

    # Write new node to head
    mov [rdi], rsi
    mov [rcx], r8

.push_post:
    # Unregister and leave
    mov qword ptr [rdx], 0
    ret

.long 0x53053053
.push_abort:
    mov qword ptr [rdx], 0
    jmp rseq_push_header

.section .data
.align 32
rseq_push_descriptor:
    .long 0, 0
    .quad .push_start
    .quad .push_post
    .quad .push_abort
