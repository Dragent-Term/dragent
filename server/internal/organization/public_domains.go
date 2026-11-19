package organization

// publicEmailDomains are free/consumer email providers — never eligible for
// domain claiming (organization_domains, 11.A.5/11.A.6), because owning an
// inbox at one of these proves nothing about controlling an organization.
// Hand-maintained rather than a vendored third-party list: this only needs
// to cover the common cases, and a plain Go map has no license to vet and
// no staleness risk from an external dependency. Add to it as real gaps
// show up in practice.
var publicEmailDomains = map[string]struct{}{
	"gmail.com":      {},
	"googlemail.com": {},
	"yahoo.com":      {},
	"yahoo.co.uk":    {},
	"yahoo.co.in":    {},
	"ymail.com":      {},
	"rocketmail.com": {},
	"outlook.com":    {},
	"hotmail.com":    {},
	"hotmail.co.uk":  {},
	"live.com":       {},
	"msn.com":        {},
	"icloud.com":     {},
	"me.com":         {},
	"mac.com":        {},
	"aol.com":        {},
	"protonmail.com": {},
	"proton.me":      {},
	"pm.me":          {},
	"zoho.com":       {},
	"gmx.com":        {},
	"gmx.net":        {},
	"mail.com":       {},
	"yandex.com":     {},
	"yandex.ru":      {},
	"mail.ru":        {},
	"qq.com":         {},
	"163.com":        {},
	"126.com":        {},
	"naver.com":      {},
	"daum.net":       {},
	"hanmail.net":    {},
	"web.de":         {},
	"t-online.de":    {},
	"libero.it":      {},
	"virgilio.it":    {},
	"orange.fr":      {},
	"laposte.net":    {},
	"seznam.cz":      {},
	"inbox.com":      {},
	"fastmail.com":   {},
	"tutanota.com":   {},
	"tutanota.de":    {},
	"hushmail.com":   {},
	"aim.com":        {},
	"comcast.net":    {},
	"verizon.net":    {},
	"att.net":        {},
	"sbcglobal.net":  {},
	"cox.net":        {},
	"btinternet.com": {},
	"sky.com":        {},
	"talktalk.net":   {},
	"rediffmail.com": {},
	"foxmail.com":    {},
	"sina.com":       {},
	"sina.cn":        {},
}

// isPublicEmailDomain reports whether domain is a known free/consumer email
// provider. Callers normalize (trim + lowercase) before checking.
func isPublicEmailDomain(domain string) bool {
	_, ok := publicEmailDomains[domain]
	return ok
}
