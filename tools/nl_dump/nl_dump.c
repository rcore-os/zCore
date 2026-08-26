#define _GNU_SOURCE

#include <errno.h>
#include <inttypes.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <net/if.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

static void die(const char *msg) {
    perror(msg);
    exit(1);
}

static void hexdump(const void *data, size_t len) {
    const unsigned char *p = (const unsigned char *)data;
    for (size_t i = 0; i < len; i++) {
        if (i && (i % 16) == 0) putchar('\n');
        printf("%02x ", p[i]);
    }
    putchar('\n');
}

static void print_mac(const unsigned char *mac, size_t len) {
    for (size_t i = 0; i < len; i++) {
        if (i) putchar(':');
        printf("%02x", mac[i]);
    }
}

static void dump_link_msg(struct nlmsghdr *nlh) {
    struct ifinfomsg *ifi = (struct ifinfomsg *)NLMSG_DATA(nlh);
    int len = nlh->nlmsg_len - NLMSG_LENGTH(sizeof(*ifi));
    struct rtattr *rta = (struct rtattr *)(((char *)ifi) + NLMSG_ALIGN(sizeof(*ifi)));

    char ifname[IFNAMSIZ] = {0};
    unsigned int mtu = 0;
    unsigned char addr[32] = {0};
    size_t addr_len = 0;

    for (; RTA_OK(rta, len); rta = RTA_NEXT(rta, len)) {
        switch (rta->rta_type) {
        case IFLA_IFNAME:
            strncpy(ifname, (const char *)RTA_DATA(rta), sizeof(ifname) - 1);
            break;
        case IFLA_MTU:
            memcpy(&mtu, RTA_DATA(rta), sizeof(mtu));
            break;
        case IFLA_ADDRESS:
            addr_len = RTA_PAYLOAD(rta);
            if (addr_len > sizeof(addr)) addr_len = sizeof(addr);
            memcpy(addr, RTA_DATA(rta), addr_len);
            break;
        default:
            break;
        }
    }

    printf("RTM_NEWLINK ifindex=%d flags=0x%x type=%u family=%u",
           ifi->ifi_index, ifi->ifi_flags, ifi->ifi_type, ifi->ifi_family);
    if (ifname[0]) printf(" ifname=%s", ifname);
    if (mtu) printf(" mtu=%u", mtu);
    if (addr_len) {
        printf(" mac=");
        print_mac(addr, addr_len);
    }
    putchar('\n');
}

static void dump_addr_msg(struct nlmsghdr *nlh) {
    struct ifaddrmsg *ifa = (struct ifaddrmsg *)NLMSG_DATA(nlh);
    int len = nlh->nlmsg_len - NLMSG_LENGTH(sizeof(*ifa));
    struct rtattr *rta = (struct rtattr *)(((char *)ifa) + NLMSG_ALIGN(sizeof(*ifa)));

    unsigned char local[16] = {0};
    size_t local_len = 0;
    char label[IFNAMSIZ] = {0};

    for (; RTA_OK(rta, len); rta = RTA_NEXT(rta, len)) {
        switch (rta->rta_type) {
        case IFA_LOCAL:
            local_len = RTA_PAYLOAD(rta);
            if (local_len > sizeof(local)) local_len = sizeof(local);
            memcpy(local, RTA_DATA(rta), local_len);
            break;
        case IFA_LABEL:
            strncpy(label, (const char *)RTA_DATA(rta), sizeof(label) - 1);
            break;
        default:
            break;
        }
    }

    printf("RTM_NEWADDR ifindex=%u family=%u prefix=%u scope=%u",
           ifa->ifa_index, ifa->ifa_family, ifa->ifa_prefixlen, ifa->ifa_scope);
    if (label[0]) printf(" label=%s", label);
    if (local_len == 4) {
        printf(" local=%u.%u.%u.%u", local[0], local[1], local[2], local[3]);
    } else if (local_len) {
        printf(" local=(");
        hexdump(local, local_len);
        printf(")");
    }
    putchar('\n');
}

static int nl_request_dump(int fd, uint16_t msg_type) {
    struct {
        struct nlmsghdr nlh;
        union {
            struct ifinfomsg ifi;
            struct ifaddrmsg ifa;
        } u;
    } req;
    memset(&req, 0, sizeof(req));
    req.nlh.nlmsg_len = NLMSG_LENGTH(sizeof(req.u));
    req.nlh.nlmsg_type = msg_type;
    req.nlh.nlmsg_flags = NLM_F_REQUEST | NLM_F_DUMP;
    req.nlh.nlmsg_seq = 1;
    req.nlh.nlmsg_pid = (uint32_t)getpid();
    // AF_UNSPEC for ifinfomsg/ifaddrmsg dumps
    req.u.ifi.ifi_family = AF_UNSPEC;
    req.u.ifa.ifa_family = AF_UNSPEC;

    struct sockaddr_nl sa;
    memset(&sa, 0, sizeof(sa));
    sa.nl_family = AF_NETLINK;

    if (sendto(fd, &req, req.nlh.nlmsg_len, 0, (struct sockaddr *)&sa, sizeof(sa)) < 0)
        return -1;
    return 0;
}

static void nl_recv_dump(int fd, uint16_t expected_done_seq) {
    char buf[8192];
    for (;;) {
        ssize_t n = recv(fd, buf, sizeof(buf), 0);
        if (n < 0) die("recv(netlink)");
        struct nlmsghdr *nlh = (struct nlmsghdr *)buf;
        for (; NLMSG_OK(nlh, (unsigned int)n); nlh = NLMSG_NEXT(nlh, n)) {
            if (nlh->nlmsg_type == NLMSG_DONE) return;
            if (nlh->nlmsg_type == NLMSG_ERROR) {
                struct nlmsgerr *e = (struct nlmsgerr *)NLMSG_DATA(nlh);
                fprintf(stderr, "NLMSG_ERROR error=%d (%s)\n", e->error, strerror(-e->error));
                return;
            }
            if (nlh->nlmsg_seq != expected_done_seq) {
                fprintf(stderr, "skip msg with seq=%u\n", nlh->nlmsg_seq);
                continue;
            }
            switch (nlh->nlmsg_type) {
            case RTM_NEWLINK:
                dump_link_msg(nlh);
                break;
            case RTM_NEWADDR:
                dump_addr_msg(nlh);
                break;
            default:
                // ignore
                break;
            }
        }
    }
}

int main(int argc, char **argv) {
    fprintf(stderr, "nl_dump argc=%d\n", argc);
    for (int i = 0; i < argc; i++) {
        fprintf(stderr, "  argv[%d]=%s\n", i, argv[i] ? argv[i] : "(null)");
    }
    if (argc != 2) {
        fprintf(stderr, "usage: %s link|addr\n", argv[0]);
        return 2;
    }
    int fd = socket(AF_NETLINK, SOCK_RAW, NETLINK_ROUTE);
    if (fd < 0) die("socket(AF_NETLINK)");

    struct sockaddr_nl local;
    memset(&local, 0, sizeof(local));
    local.nl_family = AF_NETLINK;
    local.nl_pid = (uint32_t)getpid();
    if (bind(fd, (struct sockaddr *)&local, sizeof(local)) < 0) die("bind(AF_NETLINK)");

    uint16_t t = 0;
    if (strcmp(argv[1], "link") == 0)
        t = RTM_GETLINK;
    else if (strcmp(argv[1], "addr") == 0)
        t = RTM_GETADDR;
    else {
        fprintf(stderr, "unknown cmd: %s\n", argv[1]);
        return 2;
    }

    if (nl_request_dump(fd, t) < 0) die("sendto(netlink)");
    nl_recv_dump(fd, 1);
    return 0;
}

