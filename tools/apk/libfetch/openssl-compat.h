#include <openssl/crypto.h>
#include <openssl/x509.h>
#include <openssl/x509v3.h>
#include <openssl/pem.h>
#include <openssl/ssl.h>
#include <openssl/err.h>

#ifndef X509_CHECK_FLAG_ALWAYS_CHECK_SUBJECT
#define OSSL_COMPAT_NEED_X509_CHECK 1

/* Flags for X509_check_* functions */
/* Always check subject name for host match even if subject alt names present */
#define X509_CHECK_FLAG_ALWAYS_CHECK_SUBJECT   0x1
/* Disable wildcard matching for dnsName fields and common name. */
#define X509_CHECK_FLAG_NO_WILDCARDS   0x2
/* Wildcards must not match a partial label. */
#define X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS 0x4
/* Allow (non-partial) wildcards to match multiple labels. */
#define X509_CHECK_FLAG_MULTI_LABEL_WILDCARDS 0x8
/* Constraint verifier subdomain patterns to match a single labels. */
#define X509_CHECK_FLAG_SINGLE_LABEL_SUBDOMAINS 0x10

/*
 * Match reference identifiers starting with "." to any sub-domain.
 * This is a non-public flag, turned on implicitly when the subject
 * reference identity is a DNS name.
 */
#define _X509_CHECK_FLAG_DOT_SUBDOMAINS 0x8000

int X509_check_host(X509 *x, const char *chk, size_t chklen,
    unsigned int flags, char **peername);

#endif
