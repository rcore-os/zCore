# Context switch
#
#   void swtch(struct ContextData *old, struct ContextData *new);
# 
# Save current registers in old. Load from new.	

    .text
    .globl switch
    .balign 4
switch:
        // push rip by caller

        // Save callee-save registers
        push rbx
        push rbp
        push r12
        push r13
        push r14
        push r15
        mov r15, cr3
        push r15
        
        mov [rdi], rsp      // rdi = from_rsp
        mov rsp, [rsi]      // rsi = to_rsp
        
        // Pop callee-save registers
        pop r15
        mov cr3, r15
        pop r15
        pop r14
        pop r13
        pop r12
        pop rbp
        pop rbx
        // pop rip
        ret