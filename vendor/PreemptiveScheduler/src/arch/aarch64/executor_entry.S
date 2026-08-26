# executor的入口函数
#
#   void executor_entry()
#
# 该函数只是简单的获取栈底 executor 实例地址
# 然后再跳转到 runtime.rs::run_executor 函数。
#
	.text
	.globl executor_entry
	.balign 4
executor_entry:
	ldr x0, [sp]
	b run_executor


