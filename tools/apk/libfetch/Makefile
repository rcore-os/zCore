libs-y			+= libfetch.a
libfetch.a-objs		:= common.o fetch.o http.o openssl-compat.o
CFLAGS_common.o		+= -DCA_CERT_FILE=\"$(CONFDIR)/ca.pem\" -DCA_CRL_FILE=\"$(CONFDIR)/crl.pem\"
CFLAGS_common.o		+= -DCLIENT_CERT_FILE=\"$(CONFDIR)/cert.pem\" -DCLIENT_KEY_FILE=\"$(CONFDIR)/cert.key\"

PKG_CONFIG		?= pkg-config

OPENSSL_CFLAGS		:= $(shell $(PKG_CONFIG) --cflags openssl)
OPENSSL_LIBS		:= $(shell $(PKG_CONFIG) --libs openssl)

CFLAGS_ALL		+= $(OPENSSL_CFLAGS)
