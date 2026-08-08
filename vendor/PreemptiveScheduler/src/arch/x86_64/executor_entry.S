# executor的入口函数
#
#   void executor_entry()
#
# 该函数只是简单的获取栈底的 executor 实例地址
# 然后再跳转到 runtime.rs::run_executor 函数。
#
	.text
	.global executor_entry
	.balign 4
executor_entry:
	pop rdi
	# After `switch`'s ret, RSP points at `self_addr`; this `pop` leaves RSP
	# at the exclusive stack_top (16-byte aligned) — ready for a SysV `call`.
	#
	# NEVER `push 0; jmp run_executor`: that left a null return slot under a
	# shallow stack. An IRQ + spurious RET became null-range EXECUTE with
	# [0..3]=0 and the old pop-null recover #PF-looped (+0x80). `call` pushes
	# a real return into the noreturn stub below.
	call run_executor
	# run_executor must not return; if it does, trap clearly.
	jmp executor_align_sentinel

	.balign 16
	.global executor_align_sentinel
executor_align_sentinel:
	ud2
	jmp executor_align_sentinel
