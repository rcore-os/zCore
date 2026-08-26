#include <stdlib.h>

struct qsortr_ctx {
	int (*compar)(const void *, const void *, void *);
	void *arg;
};

static __thread struct qsortr_ctx *__ctx;

static int cmp_wrapper(const void *a, const void *b)
{
	return __ctx->compar(a, b, __ctx->arg);
}

void qsort_r(void *base, size_t nmemb, size_t size,
	int (*compar)(const void *, const void *, void *),
	void *arg)
{
	struct qsortr_ctx ctx = {
		.compar = compar,
		.arg = arg,
	};
	__ctx = &ctx;
	qsort(base, nmemb, size, cmp_wrapper);
	__ctx = 0;
}
