/*
 * OUR code, not NVIDIA's -- safe "not implemented" stubs for the real
 * NVIDIA function signatures that the full vendored RM core (see
 * build.rs) references but that only matter for hardware/features
 * Eclipse's target GPU (a single desktop Turing card) does not have:
 * NVSwitch (a separate fabric ASIC), NVLink (no bridge on this SKU),
 * SPDM/Confidential-Computing attestation (off by default in NVIDIA's
 * own build: NV_USE_LIBSPDM := 0, and unsupported before Hopper
 * anyway), and the Linux ioctl control-call surface (Eclipse has no
 * userspace RM API yet). Every signature here is transcribed verbatim
 * from the real NVIDIA headers so the real, unmodified RM code that
 * calls them keeps linking -- only the bodies are ours, and every body
 * either does nothing or reports "not supported/available", matching
 * the same convention already used throughout os_interface.rs.
 */
#include <stdint.h>
#include <stdbool.h>
#include "core/core.h"
#include "os/os.h"
#include "g_allclasses.h"
#include "ctrl/ctrl0000/ctrl0000unix.h"
#include "ctrl/ctrl0080/ctrl0080unix.h"
#include "ctrl/ctrl2080/ctrl2080unix.h"
#include "virtualization/hypervisor/hypervisor.h"
#include "nvlink_errors.h"
#include "nvCpuUuid.h"
#include "hal/library/cryptlib.h"
NV_STATUS cliresCtrlCmdOsUnixCreateExportObjectFd_IMPL(struct RmClientResource *pRmCliRes, NV0000_CTRL_OS_UNIX_CREATE_EXPORT_OBJECT_FD_PARAMS *pParams)
{
    (void)pRmCliRes;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS cliresCtrlCmdOsUnixExportObjectToFd_IMPL(struct RmClientResource *pRmCliRes, NV0000_CTRL_OS_UNIX_EXPORT_OBJECT_TO_FD_PARAMS *pParams)
{
    (void)pRmCliRes;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS cliresCtrlCmdOsUnixExportObjectsToFd_IMPL(struct RmClientResource *pRmCliRes, NV0000_CTRL_OS_UNIX_EXPORT_OBJECTS_TO_FD_PARAMS *pParams)
{
    (void)pRmCliRes;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS cliresCtrlCmdOsUnixFlushUserCache_IMPL(struct RmClientResource *pRmCliRes, NV0000_CTRL_OS_UNIX_FLUSH_USER_CACHE_PARAMS *pAddressSpaceParams)
{
    (void)pRmCliRes;
    (void)pAddressSpaceParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS cliresCtrlCmdOsUnixGetExportObjectInfo_IMPL(struct RmClientResource *pRmCliRes, NV0000_CTRL_OS_UNIX_GET_EXPORT_OBJECT_INFO_PARAMS *pParams)
{
    (void)pRmCliRes;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS cliresCtrlCmdOsUnixImportObjectFromFd_IMPL(struct RmClientResource *pRmCliRes, NV0000_CTRL_OS_UNIX_IMPORT_OBJECT_FROM_FD_PARAMS *pParams)
{
    (void)pRmCliRes;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS cliresCtrlCmdOsUnixImportObjectsFromFd_IMPL(struct RmClientResource *pRmCliRes, NV0000_CTRL_OS_UNIX_IMPORT_OBJECTS_FROM_FD_PARAMS *pParams)
{
    (void)pRmCliRes;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

/* NV0000_CTRL_OS_UNIX_MEMACCT_GET/SET_LIMITS control calls (and their
 * *_PARAMS types) do not exist in the 570.144 RM -- they were added later --
 * so there is nothing to stub for this vendored release. */

NV_STATUS deviceCtrlCmdOsUnixVTGetFBInfo_IMPL(struct Device *pDevice, NV0080_CTRL_OS_UNIX_VT_GET_FB_INFO_PARAMS *pParams)
{
    (void)pDevice;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS deviceCtrlCmdOsUnixVTSwitch_IMPL(struct Device *pDevice, NV0080_CTRL_OS_UNIX_VT_SWITCH_PARAMS *pParams)
{
    (void)pDevice;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS hypervisorInjectInterrupt_IMPL(struct OBJHYPERVISOR *arg_this, VGPU_NS_INTR *arg2)
{
    (void)arg_this;
    (void)arg2;
    return NV_ERR_NOT_SUPPORTED;
}

NvBool hypervisorIsVgxHyper_IMPL(void)
{
    return NV_FALSE;
}

bool libspdm_aead_aes_gcm_decrypt_prealloc(void *context, const uint8_t *key, size_t key_size, const uint8_t *iv, size_t iv_size, const uint8_t *a_data, size_t a_data_size, const uint8_t *data_in, size_t data_in_size, const uint8_t *tag, size_t tag_size, uint8_t *data_out, size_t *data_out_size)
{
    (void)context;
    (void)key;
    (void)key_size;
    (void)iv;
    (void)iv_size;
    (void)a_data;
    (void)a_data_size;
    (void)data_in;
    (void)data_in_size;
    (void)tag;
    (void)tag_size;
    (void)data_out;
    (void)data_out_size;
    return false;
}

bool libspdm_aead_aes_gcm_encrypt_prealloc(void *context, const uint8_t *key, size_t key_size, const uint8_t *iv, size_t iv_size, const uint8_t *a_data, size_t a_data_size, const uint8_t *data_in, size_t data_in_size, uint8_t *tag_out, size_t tag_size, uint8_t *data_out, size_t *data_out_size)
{
    (void)context;
    (void)key;
    (void)key_size;
    (void)iv;
    (void)iv_size;
    (void)a_data;
    (void)a_data_size;
    (void)data_in;
    (void)data_in_size;
    (void)tag_out;
    (void)tag_size;
    (void)data_out;
    (void)data_out_size;
    return false;
}

void libspdm_aead_free(void *context)
{
    (void)context;
}

bool libspdm_aead_gcm_prealloc(void **context)
{
    (void)context;
    return false;
}

bool libspdm_asn1_get_tag(uint8_t **ptr, const uint8_t *end, size_t *length, uint32_t tag)
{
    (void)ptr;
    (void)end;
    (void)length;
    (void)tag;
    return false;
}

bool libspdm_check_crypto_backend(void)
{
    return false;
}

bool libspdm_decode_base64(const uint8_t *src, uint8_t *dst, size_t srclen, size_t *p_dstlen)
{
    (void)src;
    (void)dst;
    (void)srclen;
    (void)p_dstlen;
    return false;
}

bool libspdm_ec_compute_key(void *ec_context, const uint8_t *peer_public, size_t peer_public_size, uint8_t *key, size_t *key_size)
{
    (void)ec_context;
    (void)peer_public;
    (void)peer_public_size;
    (void)key;
    (void)key_size;
    return false;
}

void libspdm_ec_free(void *ec_context)
{
    (void)ec_context;
}

bool libspdm_ec_generate_key(void *ec_context, uint8_t *public_key, size_t *public_key_size)
{
    (void)ec_context;
    (void)public_key;
    (void)public_key_size;
    return false;
}

bool libspdm_ec_get_public_key_from_x509(const uint8_t *cert, size_t cert_size, void **ec_context)
{
    (void)cert;
    (void)cert_size;
    (void)ec_context;
    return false;
}

void * libspdm_ec_new_by_nid(size_t nid)
{
    (void)nid;
    return 0;
}

bool libspdm_ecdsa_sign(void *ec_context, size_t hash_nid, const uint8_t *message_hash, size_t hash_size, uint8_t *signature, size_t *sig_size)
{
    (void)ec_context;
    (void)hash_nid;
    (void)message_hash;
    (void)hash_size;
    (void)signature;
    (void)sig_size;
    return false;
}

bool libspdm_ecdsa_verify(void *ec_context, size_t hash_nid, const uint8_t *message_hash, size_t hash_size, const uint8_t *signature, size_t sig_size)
{
    (void)ec_context;
    (void)hash_nid;
    (void)message_hash;
    (void)hash_size;
    (void)signature;
    (void)sig_size;
    return false;
}

bool libspdm_encode_base64(const uint8_t *src, uint8_t *dst, size_t srclen, size_t *p_dstlen)
{
    (void)src;
    (void)dst;
    (void)srclen;
    (void)p_dstlen;
    return false;
}

bool libspdm_hkdf_sha256_expand(const uint8_t *prk, size_t prk_size, const uint8_t *info, size_t info_size, uint8_t *out, size_t out_size)
{
    (void)prk;
    (void)prk_size;
    (void)info;
    (void)info_size;
    (void)out;
    (void)out_size;
    return false;
}

bool libspdm_hkdf_sha256_extract(const uint8_t *key, size_t key_size, const uint8_t *salt, size_t salt_size, uint8_t *prk_out, size_t prk_out_size)
{
    (void)key;
    (void)key_size;
    (void)salt;
    (void)salt_size;
    (void)prk_out;
    (void)prk_out_size;
    return false;
}

bool libspdm_hkdf_sha384_expand(const uint8_t *prk, size_t prk_size, const uint8_t *info, size_t info_size, uint8_t *out, size_t out_size)
{
    (void)prk;
    (void)prk_size;
    (void)info;
    (void)info_size;
    (void)out;
    (void)out_size;
    return false;
}

bool libspdm_hkdf_sha384_extract(const uint8_t *key, size_t key_size, const uint8_t *salt, size_t salt_size, uint8_t *prk_out, size_t prk_out_size)
{
    (void)key;
    (void)key_size;
    (void)salt;
    (void)salt_size;
    (void)prk_out;
    (void)prk_out_size;
    return false;
}

bool libspdm_hmac_sha256_all(const void *data, size_t data_size, const uint8_t *key, size_t key_size, uint8_t *hmac_value)
{
    (void)data;
    (void)data_size;
    (void)key;
    (void)key_size;
    (void)hmac_value;
    return false;
}

bool libspdm_hmac_sha256_duplicate(const void *hmac_sha256_ctx, void *new_hmac_sha256_ctx)
{
    (void)hmac_sha256_ctx;
    (void)new_hmac_sha256_ctx;
    return false;
}

bool libspdm_hmac_sha256_final(void *hmac_sha256_ctx, uint8_t *hmac_value)
{
    (void)hmac_sha256_ctx;
    (void)hmac_value;
    return false;
}

void libspdm_hmac_sha256_free(void *hmac_sha256_ctx)
{
    (void)hmac_sha256_ctx;
}

void * libspdm_hmac_sha256_new(void)
{
    return 0;
}

bool libspdm_hmac_sha256_set_key(void *hmac_sha256_ctx, const uint8_t *key, size_t key_size)
{
    (void)hmac_sha256_ctx;
    (void)key;
    (void)key_size;
    return false;
}

bool libspdm_hmac_sha256_update(void *hmac_sha256_ctx, const void *data, size_t data_size)
{
    (void)hmac_sha256_ctx;
    (void)data;
    (void)data_size;
    return false;
}

bool libspdm_hmac_sha384_all(const void *data, size_t data_size, const uint8_t *key, size_t key_size, uint8_t *hmac_value)
{
    (void)data;
    (void)data_size;
    (void)key;
    (void)key_size;
    (void)hmac_value;
    return false;
}

bool libspdm_hmac_sha384_duplicate(const void *hmac_sha384_ctx, void *new_hmac_sha384_ctx)
{
    (void)hmac_sha384_ctx;
    (void)new_hmac_sha384_ctx;
    return false;
}

bool libspdm_hmac_sha384_final(void *hmac_sha384_ctx, uint8_t *hmac_value)
{
    (void)hmac_sha384_ctx;
    (void)hmac_value;
    return false;
}

void libspdm_hmac_sha384_free(void *hmac_sha384_ctx)
{
    (void)hmac_sha384_ctx;
}

void * libspdm_hmac_sha384_new(void)
{
    return 0;
}

bool libspdm_hmac_sha384_set_key(void *hmac_sha384_ctx, const uint8_t *key, size_t key_size)
{
    (void)hmac_sha384_ctx;
    (void)key;
    (void)key_size;
    return false;
}

bool libspdm_hmac_sha384_update(void *hmac_sha384_ctx, const void *data, size_t data_size)
{
    (void)hmac_sha384_ctx;
    (void)data;
    (void)data_size;
    return false;
}

bool libspdm_random_bytes(uint8_t *output, size_t size)
{
    (void)output;
    (void)size;
    return false;
}

void libspdm_rsa_free(void *rsa_context)
{
    (void)rsa_context;
}

bool libspdm_rsa_get_public_key_from_x509(const uint8_t *cert, size_t cert_size, void **rsa_context)
{
    (void)cert;
    (void)cert_size;
    (void)rsa_context;
    return false;
}

void * libspdm_rsa_new(void)
{
    return 0;
}

bool libspdm_rsa_pss_sign(void *rsa_context, size_t hash_nid, const uint8_t *message_hash, size_t hash_size, uint8_t *signature, size_t *sig_size)
{
    (void)rsa_context;
    (void)hash_nid;
    (void)message_hash;
    (void)hash_size;
    (void)signature;
    (void)sig_size;
    return false;
}

bool libspdm_rsa_pss_verify(void *rsa_context, size_t hash_nid, const uint8_t *message_hash, size_t hash_size, const uint8_t *signature, size_t sig_size)
{
    (void)rsa_context;
    (void)hash_nid;
    (void)message_hash;
    (void)hash_size;
    (void)signature;
    (void)sig_size;
    return false;
}

bool libspdm_rsa_set_key(void *rsa_context, const libspdm_rsa_key_tag_t key_tag, const uint8_t *big_number, size_t bn_size)
{
    (void)rsa_context;
    (void)key_tag;
    (void)big_number;
    (void)bn_size;
    return false;
}

bool libspdm_sha256_duplicate(const void *sha256_context, void *new_sha256_context)
{
    (void)sha256_context;
    (void)new_sha256_context;
    return false;
}

bool libspdm_sha256_final(void *sha256_context, uint8_t *hash_value)
{
    (void)sha256_context;
    (void)hash_value;
    return false;
}

void libspdm_sha256_free(void *sha256_context)
{
    (void)sha256_context;
}

bool libspdm_sha256_hash_all(const void *data, size_t data_size, uint8_t *hash_value)
{
    (void)data;
    (void)data_size;
    (void)hash_value;
    return false;
}

bool libspdm_sha256_init(void *sha256_context)
{
    (void)sha256_context;
    return false;
}

void * libspdm_sha256_new(void)
{
    return 0;
}

bool libspdm_sha256_update(void *sha256_context, const void *data, size_t data_size)
{
    (void)sha256_context;
    (void)data;
    (void)data_size;
    return false;
}

bool libspdm_sha384_duplicate(const void *sha384_context, void *new_sha384_context)
{
    (void)sha384_context;
    (void)new_sha384_context;
    return false;
}

bool libspdm_sha384_final(void *sha384_context, uint8_t *hash_value)
{
    (void)sha384_context;
    (void)hash_value;
    return false;
}

void libspdm_sha384_free(void *sha384_context)
{
    (void)sha384_context;
}

bool libspdm_sha384_hash_all(const void *data, size_t data_size, uint8_t *hash_value)
{
    (void)data;
    (void)data_size;
    (void)hash_value;
    return false;
}

bool libspdm_sha384_init(void *sha384_context)
{
    (void)sha384_context;
    return false;
}

void * libspdm_sha384_new(void)
{
    return 0;
}

bool libspdm_sha384_update(void *sha384_context, const void *data, size_t data_size)
{
    (void)sha384_context;
    (void)data;
    (void)data_size;
    return false;
}

int32_t libspdm_x509_compare_date_time(const void *date_time1, const void *date_time2)
{
    (void)date_time1;
    (void)date_time2;
    return -1;
}

bool libspdm_x509_get_cert_from_cert_chain(const uint8_t *cert_chain, size_t cert_chain_length, const int32_t cert_index, const uint8_t **cert, size_t *cert_length)
{
    (void)cert_chain;
    (void)cert_chain_length;
    (void)cert_index;
    (void)cert;
    (void)cert_length;
    return false;
}

bool libspdm_x509_get_extended_basic_constraints(const uint8_t *cert, size_t cert_size, uint8_t *basic_constraints, size_t *basic_constraints_size)
{
    (void)cert;
    (void)cert_size;
    (void)basic_constraints;
    (void)basic_constraints_size;
    return false;
}

bool libspdm_x509_get_extended_key_usage(const uint8_t *cert, size_t cert_size, uint8_t *usage, size_t *usage_size)
{
    (void)cert;
    (void)cert_size;
    (void)usage;
    (void)usage_size;
    return false;
}

bool libspdm_x509_get_extension_data(const uint8_t *cert, size_t cert_size, const uint8_t *oid, size_t oid_size, uint8_t *extension_data, size_t *extension_data_size)
{
    (void)cert;
    (void)cert_size;
    (void)oid;
    (void)oid_size;
    (void)extension_data;
    (void)extension_data_size;
    return false;
}

bool libspdm_x509_get_issuer_name(const uint8_t *cert, size_t cert_size, uint8_t *cert_issuer, size_t *issuer_size)
{
    (void)cert;
    (void)cert_size;
    (void)cert_issuer;
    (void)issuer_size;
    return false;
}

bool libspdm_x509_get_key_usage(const uint8_t *cert, size_t cert_size, size_t *usage)
{
    (void)cert;
    (void)cert_size;
    (void)usage;
    return false;
}

bool libspdm_x509_get_serial_number(const uint8_t *cert, size_t cert_size, uint8_t *serial_number, size_t *serial_number_size)
{
    (void)cert;
    (void)cert_size;
    (void)serial_number;
    (void)serial_number_size;
    return false;
}

bool libspdm_x509_get_signature_algorithm(const uint8_t *cert, size_t cert_size, uint8_t *oid, size_t *oid_size)
{
    (void)cert;
    (void)cert_size;
    (void)oid;
    (void)oid_size;
    return false;
}

bool libspdm_x509_get_subject_name(const uint8_t *cert, size_t cert_size, uint8_t *cert_subject, size_t *subject_size)
{
    (void)cert;
    (void)cert_size;
    (void)cert_subject;
    (void)subject_size;
    return false;
}

bool libspdm_x509_get_validity(const uint8_t *cert, size_t cert_size, uint8_t *from, size_t *from_size, uint8_t *to, size_t *to_size)
{
    (void)cert;
    (void)cert_size;
    (void)from;
    (void)from_size;
    (void)to;
    (void)to_size;
    return false;
}

bool libspdm_x509_get_version(const uint8_t *cert, size_t cert_size, size_t *version)
{
    (void)cert;
    (void)cert_size;
    (void)version;
    return false;
}

bool libspdm_x509_set_date_time(const char *date_time_str, void *date_time, size_t *date_time_size)
{
    (void)date_time_str;
    (void)date_time;
    (void)date_time_size;
    return false;
}

bool libspdm_x509_verify_cert(const uint8_t *cert, size_t cert_size, const uint8_t *ca_cert, size_t ca_cert_size)
{
    (void)cert;
    (void)cert_size;
    (void)ca_cert;
    (void)ca_cert_size;
    return false;
}

bool libspdm_x509_verify_cert_chain(const uint8_t *root_cert, size_t root_cert_length, const uint8_t *cert_chain, size_t cert_chain_length)
{
    (void)root_cert;
    (void)root_cert_length;
    (void)cert_chain;
    (void)cert_chain_length;
    return false;
}

void nvlink_acquireLock(void *)
{

}

NvlStatus nvlink_acquire_fabric_mgmt_cap(void *osPrivate, NvU64 capDescriptor)
{
    (void)osPrivate;
    (void)capDescriptor;
    return -1; /* NVL_ERR_GENERIC */
}

void * nvlink_allocLock(void)
{
    return 0;
}

void nvlink_assert(int expression)
{
    (void)expression;
}

void nvlink_free(void *)
{

}

void nvlink_freeLock(void *)
{

}

NvU64 nvlink_get_platform_time(void)
{
    return 0;
}

int nvlink_is_admin(void)
{
    return -1;
}

int nvlink_is_fabric_manager(void *osPrivate)
{
    (void)osPrivate;
    return -1;
}

void * nvlink_malloc(NvLength)
{
    return 0;
}

void * nvlink_memcpy(void *, const void *, NvLength)
{
    return 0;
}

void * nvlink_memset(void *, int, NvLength)
{
    return 0;
}

void nvlink_releaseLock(void *)
{

}

void nvlink_sleep(unsigned int ms)
{
    (void)ms;
}

int nvlink_strcmp(const char *, const char *)
{
    return -1;
}

char * nvlink_strcpy(char *, const char *)
{
    return 0;
}

NvLength nvlink_strlen(const char *)
{
    return 0;
}

NV_STATUS subdeviceCtrlCmdOsUnixAllowDisallowGcoff_IMPL(struct Subdevice *pSubdevice, NV2080_CTRL_OS_UNIX_ALLOW_DISALLOW_GCOFF_PARAMS *pParams)
{
    (void)pSubdevice;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS subdeviceCtrlCmdOsUnixAudioDynamicPower_IMPL(struct Subdevice *pSubdevice, NV2080_CTRL_OS_UNIX_AUDIO_DYNAMIC_POWER_PARAMS *pParams)
{
    (void)pSubdevice;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS subdeviceCtrlCmdOsUnixGc6BlockerRefCnt_IMPL(struct Subdevice *pSubdevice, NV2080_CTRL_OS_UNIX_GC6_BLOCKER_REFCNT_PARAMS *pParams)
{
    (void)pSubdevice;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS subdeviceCtrlCmdOsUnixUpdateTgpStatus_IMPL(struct Subdevice *pSubdevice, NV2080_CTRL_OS_UNIX_UPDATE_TGP_STATUS_PARAMS *pParams)
{
    (void)pSubdevice;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NV_STATUS subdeviceCtrlCmdOsUnixVidmemPersistenceStatus_IMPL(struct Subdevice *pSubdevice, NV2080_CTRL_OS_UNIX_VIDMEM_PERSISTENCE_STATUS_PARAMS *pParams)
{
    (void)pSubdevice;
    (void)pParams;
    return NV_ERR_NOT_SUPPORTED;
}

NvlStatus nvswitch_os_acquire_fabric_mgmt_cap(void *osPrivate, NvU64 capDescriptor)
{
    (void)osPrivate;
    (void)capDescriptor;
    return -1; /* NVL_ERR_GENERIC */
}

NvlStatus nvswitch_os_add_client_event(void *osHandle, void *osPrivate, NvU32 eventId)
{
    (void)osHandle;
    (void)osPrivate;
    (void)eventId;
    return -1; /* NVL_ERR_GENERIC */
}

NvlStatus nvswitch_os_alloc_contig_memory(void *os_handle, void **virt_addr, NvU32 size, NvBool force_dma32)
{
    (void)os_handle;
    (void)virt_addr;
    (void)size;
    (void)force_dma32;
    return -1; /* NVL_ERR_GENERIC */
}

void nvswitch_os_assert_log(const char *pFormat, ...)
{
    (void)pFormat;
}

void nvswitch_os_free(void *pMem)
{
    (void)pMem;
}

void nvswitch_os_free_contig_memory(void *os_handle, void *virt_addr, NvU32 size)
{
    (void)os_handle;
    (void)virt_addr;
    (void)size;
}

NvlStatus nvswitch_os_get_os_version(NvU32 *pMajorVer, NvU32 *pMinorVer, NvU32 *pBuildNum)
{
    (void)pMajorVer;
    (void)pMinorVer;
    (void)pBuildNum;
    return -1; /* NVL_ERR_GENERIC */
}

NvlStatus nvswitch_os_get_pid(NvU32 *pPid)
{
    (void)pPid;
    return -1; /* NVL_ERR_GENERIC */
}

NvU64 nvswitch_os_get_platform_time(void)
{
    return 0;
}

NvU64 nvswitch_os_get_platform_time_epoch(void)
{
    return 0;
}

NvlStatus nvswitch_os_get_supported_register_events_params(NvBool *bSupportsManyEvents, NvBool *bUserSuppliesOsData)
{
    (void)bSupportsManyEvents;
    (void)bUserSuppliesOsData;
    return -1; /* NVL_ERR_GENERIC */
}

int nvswitch_os_is_admin(void)
{
    return -1;
}

int nvswitch_os_is_fabric_manager(void *osPrivate)
{
    (void)osPrivate;
    return -1;
}

NvBool nvswitch_os_is_uuid_in_blacklist(NvUuid *uuid)
{
    (void)uuid;
    return NV_FALSE;
}

void * nvswitch_os_malloc_trace(NvLength size, const char *file, NvU32 line)
{
    (void)size;
    (void)file;
    (void)line;
    return 0;
}

NvlStatus nvswitch_os_map_dma_region(void *os_handle, void *cpu_addr, NvU64 *dma_handle, NvU32 size, NvU32 direction)
{
    (void)os_handle;
    (void)cpu_addr;
    (void)dma_handle;
    (void)size;
    (void)direction;
    return -1; /* NVL_ERR_GENERIC */
}

NvU32 nvswitch_os_mem_read32(const volatile void * pAddress)
{
    (void)pAddress;
    return 0;
}

void nvswitch_os_mem_write32(volatile void *pAddress, NvU32 data)
{
    (void)pAddress;
    (void)data;
}

void * nvswitch_os_memcpy(void *pDest, const void *pSrc, NvLength size)
{
    (void)pDest;
    (void)pSrc;
    (void)size;
    return 0;
}

void * nvswitch_os_memset(void *pDest, int value, NvLength size)
{
    (void)pDest;
    (void)value;
    (void)size;
    return 0;
}

NvlStatus nvswitch_os_notify_client_event(void *osHandle, void *osPrivate, NvU32 eventId)
{
    (void)osHandle;
    (void)osPrivate;
    (void)eventId;
    return -1; /* NVL_ERR_GENERIC */
}

void nvswitch_os_override_platform(void *os_handle, NvBool *rtlsim)
{
    (void)os_handle;
    (void)rtlsim;
}

NvlStatus nvswitch_os_read_registry_dword(void *os_handle, const char *name, NvU32 *data)
{
    (void)os_handle;
    (void)name;
    (void)data;
    return -1; /* NVL_ERR_GENERIC */
}

NvlStatus nvswitch_os_remove_client_event(void *osHandle, void *osPrivate)
{
    (void)osHandle;
    (void)osPrivate;
    return -1; /* NVL_ERR_GENERIC */
}

NvlStatus nvswitch_os_set_dma_mask(void *os_handle, NvU32 dma_addr_width)
{
    (void)os_handle;
    (void)dma_addr_width;
    return -1; /* NVL_ERR_GENERIC */
}

void nvswitch_os_sleep(unsigned int ms)
{
    (void)ms;
}

int nvswitch_os_snprintf(char *pString, NvLength size, const char *pFormat, ...)
{
    (void)pString;
    (void)size;
    (void)pFormat;
    return -1;
}

NvLength nvswitch_os_strlen(const char *str)
{
    (void)str;
    return 0;
}

int nvswitch_os_strncmp(const char *s1, const char *s2, NvLength length)
{
    (void)s1;
    (void)s2;
    (void)length;
    return -1;
}

char* nvswitch_os_strncpy(char *pDest, const char *pSrc, NvLength length)
{
    (void)pDest;
    (void)pSrc;
    (void)length;
    return 0;
}

NvlStatus nvswitch_os_sync_dma_region_for_cpu(void *os_handle, NvU64 dma_handle, NvU32 size, NvU32 direction)
{
    (void)os_handle;
    (void)dma_handle;
    (void)size;
    (void)direction;
    return -1; /* NVL_ERR_GENERIC */
}

NvlStatus nvswitch_os_sync_dma_region_for_device(void *os_handle, NvU64 dma_handle, NvU32 size, NvU32 direction)
{
    (void)os_handle;
    (void)dma_handle;
    (void)size;
    (void)direction;
    return -1; /* NVL_ERR_GENERIC */
}

NvlStatus nvswitch_os_unmap_dma_region(void *os_handle, void *cpu_addr, NvU64 dma_handle, NvU32 size, NvU32 direction)
{
    (void)os_handle;
    (void)cpu_addr;
    (void)dma_handle;
    (void)size;
    (void)direction;
    return -1; /* NVL_ERR_GENERIC */
}

int nvswitch_os_vsnprintf(char *buf, NvLength size, const char *fmt, va_list arglist)
{
    (void)buf;
    (void)size;
    (void)fmt;
    (void)arglist;
    return -1;
}

void nvswitch_os_print(int log_level, const char *pFormat, ...)
{
    (void)log_level;
    (void)pFormat;
}

bool libspdm_aead_aes_gcm_encrypt(const uint8_t *key, size_t key_size, const uint8_t *iv, size_t iv_size, const uint8_t *a_data, size_t a_data_size, const uint8_t *data_in, size_t data_in_size, uint8_t *tag_out, size_t tag_size, uint8_t *data_out, size_t *data_out_size)
{
    (void)key;
    (void)key_size;
    (void)iv;
    (void)iv_size;
    (void)a_data;
    (void)a_data_size;
    (void)data_in;
    (void)data_in_size;
    (void)tag_out;
    (void)tag_size;
    (void)data_out;
    (void)data_out_size;
    return false;
}

bool libspdm_aead_aes_gcm_decrypt(const uint8_t *key, size_t key_size, const uint8_t *iv, size_t iv_size, const uint8_t *a_data, size_t a_data_size, const uint8_t *data_in, size_t data_in_size, const uint8_t *tag, size_t tag_size, uint8_t *data_out, size_t *data_out_size)
{
    (void)key;
    (void)key_size;
    (void)iv;
    (void)iv_size;
    (void)a_data;
    (void)a_data_size;
    (void)data_in;
    (void)data_in_size;
    (void)tag;
    (void)tag_size;
    (void)data_out;
    (void)data_out_size;
    return false;
}

void nvswitch_os_report_error(void *os_handle, NvU32 error_code, const char *fmt, ...)
{
    (void)os_handle;
    (void)error_code;
    (void)fmt;
}
