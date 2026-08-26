#include "apk_repoparser.h"
#include "apk_ctype.h"
#include "apk_print.h"
#include "apk_pathbuilder.h"

struct apk_variable {
	struct hlist_node hash_node;
	apk_blob_t value;
	uint8_t flags;
	uint8_t keylen;
	char key[];
};

static apk_blob_t variable_hash_get_key(apk_hash_item item)
{
	struct apk_variable *var = item;
	return APK_BLOB_PTR_LEN(var->key, var->keylen);
}

static void variable_hash_delete_item(apk_hash_item item)
{
	struct apk_variable *var = item;
	free(var->value.ptr);
	free(var);
}

static struct apk_hash_ops variable_ops = {
	.node_offset = offsetof(struct apk_variable, hash_node),
	.get_key = variable_hash_get_key,
	.hash_key = apk_blob_hash,
	.compare = apk_blob_compare,
	.delete_item = variable_hash_delete_item,
};

int apk_variable_set(struct apk_hash *vars, apk_blob_t key, apk_blob_t value, uint8_t flags)
{
	unsigned long hash = apk_hash_from_key(vars, key);
	struct apk_variable *var = apk_hash_get_hashed(vars, key, hash);

	if (!var) {
		var = malloc(sizeof *var + key.len);
		if (!var) return -ENOMEM;
		var->keylen = key.len;
		memcpy(var->key, key.ptr, key.len);
		apk_hash_insert_hashed(vars, var, hash);
	} else {
		if (!(flags & APK_VARF_OVERWRITE)) return 0;
		if (var->flags & APK_VARF_READONLY) return 0;
		free(var->value.ptr);
	}
	var->flags = flags;
	var->value = apk_blob_dup(value);
	return 0;
}

static int apk_variable_subst(void *ctx, apk_blob_t key, apk_blob_t *to)
{
	struct apk_hash *vars = ctx;
	struct apk_variable *var = apk_hash_get(vars, key);
	if (!var) return -APKE_REPO_VARIABLE;
	apk_blob_push_blob(to, var->value);
	return 0;
}

enum {
	APK_REPOTYPE_OMITTED,
	APK_REPOTYPE_NDX,
	APK_REPOTYPE_V2,
	APK_REPOTYPE_V3,
};

static bool get_word(apk_blob_t *line, apk_blob_t *word)
{
	apk_blob_cspn(*line, APK_CTYPE_REPOSITORY_SEPARATOR, word, line);
	apk_blob_spn(*line, APK_CTYPE_REPOSITORY_SEPARATOR, NULL, line);
	return word->len > 0;
}

void apk_repoparser_init(struct apk_repoparser *rp, struct apk_out *out, const struct apk_repoparser_ops *ops)
{
	*rp = (struct apk_repoparser) {
		.out = out,
		.ops = ops,
	};
	apk_hash_init(&rp->variables, &variable_ops, 10);
}

void apk_repoparser_free(struct apk_repoparser *rp)
{
	apk_hash_free(&rp->variables);
}

void apk_repoparser_set_file(struct apk_repoparser *rp, const char *file)
{
	rp->file = file;
	rp->line = 0;
}

static int apk_repoparser_subst(void *ctx, apk_blob_t key, apk_blob_t *to)
{
	struct apk_repoparser *rp = ctx;
	int r = apk_variable_subst(&rp->variables, key, to);
	if (r < 0) apk_warn(rp->out, "%s:%d: undefined variable: " BLOB_FMT,
		rp->file, rp->line, BLOB_PRINTF(key));
	return r;
}

static int apk_repoparser_parse_set(struct apk_repoparser *rp, apk_blob_t line)
{
	char buf[PATH_MAX];
	apk_blob_t key, value;
	uint8_t flags = APK_VARF_OVERWRITE;

	while (line.len && line.ptr[0] == '-') {
		get_word(&line, &key);
		if (apk_blob_compare(key, APK_BLOB_STRLIT("-default")) == 0)
			flags &= ~APK_VARF_OVERWRITE;
		else {
			apk_warn(rp->out, "%s:%d: invalid option: " BLOB_FMT,
				rp->file, rp->line, BLOB_PRINTF(key));
			return -APKE_REPO_SYNTAX;
		}
	}

	if (!apk_blob_split(line, APK_BLOB_STRLIT("="), &key, &value) ||
	    apk_blob_starts_with(key, APK_BLOB_STRLIT("APK_")) ||
	    !isalpha(key.ptr[0]) || apk_blob_spn(key, APK_CTYPE_VARIABLE_NAME, NULL, NULL)) {
		apk_warn(rp->out, "%s:%d: invalid variable definition: " BLOB_FMT, rp->file, rp->line, BLOB_PRINTF(line));
		return -APKE_REPO_VARIABLE;
	}

	int r = apk_blob_subst(buf, sizeof buf, value, apk_repoparser_subst, rp);
	if (r < 0) return r;

	return apk_variable_set(&rp->variables, key, APK_BLOB_PTR_LEN(buf, r), flags);
}

static bool is_url(apk_blob_t word)
{
	return word.ptr[0] == '/' || apk_blob_contains(word, APK_BLOB_STRLIT("://")) > 0;
}

static bool is_keyword(apk_blob_t word)
{
	if (word.ptr[0] == '@') return false; // tag
	return !is_url(word);
}

int apk_repoparser_parse(struct apk_repoparser *rp, apk_blob_t line, bool allow_keywords)
{
	struct apk_pathbuilder pb;
	struct apk_out *out = rp->out;
	apk_blob_t word, tag = APK_BLOB_NULL;
	int type = APK_REPOTYPE_OMITTED;

	rp->line++;
	if (!line.ptr || line.len == 0 || line.ptr[0] == '#') return 0;

	if (!get_word(&line, &word)) return -APKE_REPO_SYNTAX;
	if (allow_keywords && is_keyword(word)) {
		if (apk_blob_compare(word, APK_BLOB_STRLIT("set")) == 0)
			return apk_repoparser_parse_set(rp, line);
		if (apk_blob_compare(word, APK_BLOB_STRLIT("ndx")) == 0)
			type = APK_REPOTYPE_NDX;
		else if (apk_blob_compare(word, APK_BLOB_STRLIT("v2")) == 0)
			type = APK_REPOTYPE_V2;
		else if (apk_blob_compare(word, APK_BLOB_STRLIT("v3")) == 0)
			type = APK_REPOTYPE_V3;
		else {
			apk_warn(out, "%s:%d: unrecogized keyword: " BLOB_FMT,
				rp->file, rp->line, BLOB_PRINTF(word));
			return -APKE_REPO_KEYWORD;
		}
		if (!get_word(&line, &word)) return -APKE_REPO_SYNTAX;
	}

	if (word.ptr[0] == '@') {
		tag = word;
		if (!get_word(&line, &word)) return -APKE_REPO_SYNTAX;
	}
	if (type == APK_REPOTYPE_OMITTED) {
		if (apk_blob_ends_with(word, APK_BLOB_STRLIT(".adb")) ||
		    apk_blob_ends_with(word, APK_BLOB_STRLIT(".tar.gz")))
			type = APK_REPOTYPE_NDX;
		else
			type = APK_REPOTYPE_V2;
	}
	const char *index_file = NULL;
	switch (type) {
	case APK_REPOTYPE_V2:
		index_file = "APKINDEX.tar.gz";
		break;
	case APK_REPOTYPE_V3:
		index_file = "Packages.adb";
		break;
	}

	char urlbuf[PATH_MAX], compbuf[PATH_MAX];;
	int r = apk_blob_subst(urlbuf, sizeof urlbuf, word, apk_repoparser_subst, rp);
	if (r < 0) return r;

	apk_blob_t url = apk_blob_trim_end(APK_BLOB_PTR_LEN(urlbuf, r), '/');
	apk_blob_t components = line;
	if (allow_keywords && !is_url(url)) {
		apk_warn(out, "%s:%d: invalid url: " BLOB_FMT,
			rp->file, rp->line, BLOB_PRINTF(url));
		return -APKE_REPO_SYNTAX;
	}
	if (!components.len) return rp->ops->repository(rp, url, index_file, tag);

	r = apk_blob_subst(compbuf, sizeof compbuf, components, apk_repoparser_subst, rp);
	if (r < 0) return r;

	components = APK_BLOB_PTR_LEN(compbuf, r);
	apk_pathbuilder_setb(&pb, url);
	apk_blob_foreach_word(component, components) {
		int n = apk_pathbuilder_pushb(&pb, component);
		r = rp->ops->repository(rp, apk_pathbuilder_get(&pb), index_file, tag);
		if (r) return r;
		apk_pathbuilder_pop(&pb, n);
	}
	return 0;
}
