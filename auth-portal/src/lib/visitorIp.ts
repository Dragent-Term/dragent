import { BlockList, isIP } from "node:net";

// CloudFront's origin-facing addresses (AWS ip-ranges.json, service
// CLOUDFRONT_ORIGIN_FACING, fetched 2026-11-17). They are hops inside our hosting
// edge, never a visitor.
const CLOUDFRONT_ORIGIN_FACING_V4 = [
  "13.124.199.0/24", "130.176.0.0/18", "130.176.128.0/21", "130.176.136.0/23",
  "130.176.140.0/22", "130.176.144.0/20", "130.176.160.0/19", "130.176.192.0/19",
  "130.176.64.0/21", "130.176.72.0/22", "130.176.76.0/24", "130.176.78.0/23",
  "130.176.80.0/22", "130.176.86.0/23", "130.176.88.0/21", "130.176.96.0/19",
  "15.158.0.0/16", "18.68.0.0/16", "204.246.166.0/24", "205.251.218.0/24",
  "24.110.128.0/17", "3.172.0.0/18", "3.172.64.0/18", "3.29.57.0/26",
  "51.72.40.128/28", "51.72.40.144/28", "51.72.96.0/23", "51.74.20.0/24",
  "51.74.21.0/24", "51.74.22.0/24", "52.46.0.0/18", "52.82.128.0/23", "52.82.134.0/23",
  "54.182.128.0/20", "54.182.144.0/21", "54.182.154.0/23", "54.182.156.0/22",
  "54.182.160.0/21", "54.182.172.0/22", "54.182.176.0/21", "54.182.184.0/22",
  "54.182.188.0/23", "54.182.224.0/21", "54.182.240.0/21", "54.182.248.0/22",
  "54.239.134.0/23", "54.239.170.0/23", "54.239.204.0/22", "54.239.208.0/21",
  "64.252.128.0/18", "64.252.64.0/18", "70.132.0.0/18",
];
const CLOUDFRONT_ORIGIN_FACING_V6 = [
  "2001:3fc6:3c::/47", "2001:3fc6:4::/48", "2001:3fc6:5::/48",
  "2406:da10:847f:a100::/56", "2406:da11:438:2300::/56", "2406:da12:4b:c200::/56",
  "2406:da12:8b2e:9c00::/56", "2406:da14:17bd:2f00::/56", "2406:da14:80bb:ea00::/56",
  "2406:da16:c01:c200::/56", "2406:da18:9fa:1b00::/56", "2406:da19:e19:4a00::/56",
  "2406:da1a:6df:6c00::/56", "2406:da1b:e7c:ad00::/56", "2406:da1c:787:2b00::/56",
  "2406:da1c:8d8c:4600::/56", "2406:da1e:705:1600::/56", "2406:da1f:396:9100::/56",
  "2600:1f11:e79:a800::/56", "2600:1f13:417:4d00::/56", "2600:1f16:1923:3000::/56",
  "2600:1f17:4356:f100::/56", "2600:1f18:7530:7200::/56", "2600:1f1a:4568:b500::/56",
  "2600:1f1c:da:600::/56", "2600:1f1e:a7:2300::/56", "2600:9000:1000::/36",
  "2600:9000:5200::/40", "2600:9000:6000::/36", "2a05:d011:531:1800::/56",
  "2a05:d012:581:b400::/56", "2a05:d014:1362:b000::/56", "2a05:d016:9ed:de00::/56",
  "2a05:d018:1a3a:2d00::/56", "2a05:d019:80b:e300::/56", "2a05:d01a:8a9:be00::/56",
  "2a05:d01c:343:c300::/56", "2a05:d025:e59:fb00::/56",
];

// Addresses that are never a visitor on the internet: our edge's own hops,
// plus private, loopback, link-local, shared (CGNAT) and unique-local ranges.
const notVisitors = new BlockList();
for (const cidr of CLOUDFRONT_ORIGIN_FACING_V4) addCidr(cidr, "ipv4");
for (const cidr of CLOUDFRONT_ORIGIN_FACING_V6) addCidr(cidr, "ipv6");
for (const cidr of [
  "0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16",
  "172.16.0.0/12", "192.168.0.0/16",
]) addCidr(cidr, "ipv4");
for (const cidr of ["::/128", "::1/128", "fc00::/7", "fe80::/10"]) addCidr(cidr, "ipv6");

function addCidr(cidr: string, type: "ipv4" | "ipv6") {
  const [net, bits] = cidr.split("/");
  notVisitors.addSubnet(net, Number(bits), type);
}

function normalize(raw: string): string {
  let ip = raw.trim();
  // "[2001:db8::1]:443" or "203.0.113.9:443"
  const bracketed = ip.match(/^\[([^\]]+)\](?::\d+)?$/);
  if (bracketed) ip = bracketed[1];
  else if (/^\d+\.\d+\.\d+\.\d+:\d+$/.test(ip)) ip = ip.replace(/:\d+$/, "");
  const mapped = ip.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/i);
  if (mapped) ip = mapped[1];
  return ip;
}

/**
 * The visitor's address, as our hosting edge (CloudFront, under Amplify)
 * recorded it. CloudFront appends the address it saw to X-Forwarded-For,
 * so the visitor is the last entry that isn't one of the edge's own hops or
 * a private address. Entries before it were sent by the caller and are
 * ignored, as is cf-connecting-ip: we aren't behind Cloudflare, so a caller
 * can set it to anything. Returns "" when no public address is found; the
 * API then counts the request against this server's address instead.
 */
export function visitorIP(headers: Headers): string {
  const entries = headers
    .get("x-forwarded-for")
    ?.split(",")
    .map(normalize)
    .filter(Boolean) ?? [];
  for (let i = entries.length - 1; i >= 0; i--) {
    const ip = entries[i];
    const type = isIP(ip);
    if (type === 0) continue;
    if (notVisitors.check(ip, type === 4 ? "ipv4" : "ipv6")) continue;
    return ip;
  }
  return "";
}
