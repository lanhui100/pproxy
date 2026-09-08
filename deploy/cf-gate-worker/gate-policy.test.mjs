// gate-policy 单测：node deploy/cf-gate-worker/gate-policy.test.mjs
import {
  DEFAULT_BLOCKED_COLOS,
  DEFAULT_ALLOWED_COLOS,
  DEFAULT_ALLOWED_EGRESS_COUNTRIES,
  COMPLIANT_EGRESS_SUFFIXES,
  isGoogleHost,
  parseBlockedColos,
  parseAllowedColos,
  parseAllowedEgressCountries,
  requiresCompliantEgress,
  shouldBlockColo,
  shouldBlockEgress,
  strictGoogleEnabled,
} from './gate-policy.mjs'

let passed = 0
let failed = 0
function check(name, cond, extra = '') {
  if (cond) {
    passed++
    console.log(`  [pass] ${name}`)
  } else {
    failed++
    console.log(`  [fail] ${name} ${extra}`)
  }
}

console.log('[1] Google host 判定')
check('oauth2.googleapis.com 命中', isGoogleHost('oauth2.googleapis.com'))
check('daily-cloudcode-pa.googleapis.com 命中', isGoogleHost('daily-cloudcode-pa.googleapis.com'))
check('generativelanguage.googleapis.com 命中', isGoogleHost('generativelanguage.googleapis.com'))
check('accounts.google.com 命中', isGoogleHost('accounts.google.com'))
check('deepmind.google 命中', isGoogleHost('deepmind.google'))
check('antigravity.google 命中（agy 官方域名）', isGoogleHost('antigravity.google'))
check('api.antigravity.google 命中（agy 子域）', isGoogleHost('api.antigravity.google'))
check('labs.google 命中（agy/实验产品域名）', isGoogleHost('labs.google'))
check('www.gstatic.com 命中', isGoogleHost('www.gstatic.com'))
check('大写归一', isGoogleHost('OAuth2.GOOGLEAPIS.COM'))
check('末尾点归一', isGoogleHost('googleapis.com.'))
check('youtube.com 不命中（独立非 API 业务）', !isGoogleHost('www.youtube.com'))
check('github.com 不命中', !isGoogleHost('github.com'))
check('notgoogleapis.com 不命中（dot-boundary）', !isGoogleHost('notgoogleapis.com'))
check('googleapis.com.evil.cn 不命中', !isGoogleHost('googleapis.com.evil.cn'))
check('googleapis.com.hk 不命中（无区域别名）', !isGoogleHost('googleapis.com.hk'))
check('空 host 不命中', !isGoogleHost(''))

console.log('[2] colo 门禁判定 — 官方不支持与黑名单区域')
check('HKG + google host → 拒', shouldBlockColo('HKG', 'oauth2.googleapis.com') === 'unsupported_colo:HKG')
check('MFM + google host → 拒', shouldBlockColo('MFM', 'google.com') === 'unsupported_colo:MFM')
check('PEK (北京) + google host → 拒', shouldBlockColo('PEK', 'daily-cloudcode-pa.googleapis.com') === 'unsupported_colo:PEK')
check('PVG (上海) + google host → 拒', shouldBlockColo('PVG', 'google.com') === 'unsupported_colo:PVG')
check('CAN (广州) + google host → 拒', shouldBlockColo('CAN', 'google.com') === 'unsupported_colo:CAN')
check('DME (莫斯科) + google host → 拒', shouldBlockColo('DME', 'google.com') === 'unsupported_colo:DME')

console.log('[3] colo 门禁判定 — 非 Google 流量全量放行（P0-2 防全量倾泻）')
check('HKG + youtube → 放行', shouldBlockColo('HKG', 'www.youtube.com') === null)
check('HKG + github → 放行', shouldBlockColo('HKG', 'github.com') === null)
check('PEK + openai → 放行', shouldBlockColo('PEK', 'openai.com') === null)
check('MFM + claude.ai → 放行', shouldBlockColo('MFM', 'claude.ai') === null)

console.log('[4] colo 门禁判定 — 官方支持与合规区域')
check('IAD (美东) + google host → 放行', shouldBlockColo('IAD', 'oauth2.googleapis.com') === null)
check('SJC (美西硅谷) + google host → 放行', shouldBlockColo('SJC', 'oauth2.googleapis.com') === null)
check('LAX (洛杉矶) + google host → 放行', shouldBlockColo('LAX', 'google.com') === null)
check('LHR (伦敦) + google host → 放行', shouldBlockColo('LHR', 'google.com') === null)
check('FRA (法兰克福) + google host → 放行', shouldBlockColo('FRA', 'google.com') === null)
check('NRT (东京) + google host → 放行', shouldBlockColo('NRT', 'daily-cloudcode-pa.googleapis.com') === null)
check('SIN (新加坡) + google host → 放行', shouldBlockColo('SIN', 'daily-cloudcode-pa.googleapis.com') === null)

console.log('[5] colo 缺失与大小写归一')
check('colo 缺失针对 Google fail-secure 拒', shouldBlockColo(undefined, 'oauth2.googleapis.com') === 'unsupported_colo:MISSING')
check('colo 空串针对 Google fail-secure 拒', shouldBlockColo('', 'oauth2.googleapis.com') === 'unsupported_colo:MISSING')
check('colo 缺失针对非 Google 放行', shouldBlockColo(undefined, 'github.com') === null)
check('colo 小写归一', shouldBlockColo('hkg', 'google.com') === 'unsupported_colo:HKG')

console.log('[6] 严格白名单模式 (strictWhitelist)')
check(
  '严格白名单模式下未知地区被拒',
  shouldBlockColo('XYZ', 'daily-cloudcode-pa.googleapis.com', { strictWhitelist: true }) === 'unsupported_colo:XYZ'
)
check(
  '严格白名单模式下合规美区放行',
  shouldBlockColo('IAD', 'daily-cloudcode-pa.googleapis.com', { strictWhitelist: true }) === null
)
check(
  '严格白名单模式下非 Google host 仍放行',
  shouldBlockColo('XYZ', 'github.com', { strictWhitelist: true }) === null
)

console.log('[7] 环境变量解析与覆盖')
check('默认黑名单包含 HKG, MFM, PEK', DEFAULT_BLOCKED_COLOS.includes('HKG') && DEFAULT_BLOCKED_COLOS.includes('PEK'))
check('默认白名单包含 IAD, SJC, NRT', DEFAULT_ALLOWED_COLOS.includes('IAD') && DEFAULT_ALLOWED_COLOS.includes('NRT'))
check('env 黑名单解析', parseBlockedColos('hkg, pek, can').join(',') === 'HKG,PEK,CAN')
check('env 白名单解析', parseAllowedColos('iad, sjx').join(',') === 'IAD,SJX')

console.log('[8] 严格白名单默认开启（P1-2 fail-closed）')
check('env 未设置 → 严格开启', strictGoogleEnabled(undefined) === true)
check('env 空串 → 严格开启', strictGoogleEnabled('') === true)
check('env=true → 严格开启', strictGoogleEnabled('true') === true)
check('env=1 → 严格开启', strictGoogleEnabled('1') === true)
check('env=false → 显式关闭', strictGoogleEnabled('false') === false)
check('env=0 → 显式关闭', strictGoogleEnabled('0') === false)

console.log('[9] 合规出口 host 判定（A/B 专项）')
check('daily-cloudcode-pa.googleapis.com 要求合规出口', requiresCompliantEgress('daily-cloudcode-pa.googleapis.com'))
check('cloudcode-pa.googleapis.com 要求合规出口', requiresCompliantEgress('cloudcode-pa.googleapis.com'))
check('cloudaicompanion.googleapis.com 要求合规出口', requiresCompliantEgress('cloudaicompanion.googleapis.com'))
check('大写与末尾点归一化', requiresCompliantEgress('DAILY-CLOUDCODE-PA.GOOGLEAPIS.COM.'))
check('认证类不要求合规出口', !requiresCompliantEgress('oauth2.googleapis.com'))
check('泛 Google 不要求合规出口', !requiresCompliantEgress('generativelanguage.googleapis.com'))
check('仿冒域名不命中', !requiresCompliantEgress('cloudcode-pa.googleapis.com.evil.cn'))
check('空 host 不命中', !requiresCompliantEgress(''))
check('Rust 侧清单口径一致（3 项）', COMPLIANT_EGRESS_SUFFIXES.length === 3)

console.log('[10] 出站地理门禁（fail-closed）')
check('合规地区放行', shouldBlockEgress({ country: 'US', status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com') === null)
check('合规地区放行（JP）', shouldBlockEgress({ country: 'jp', status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com') === null)
check('非合规地区拒绝并带国家码', shouldBlockEgress({ country: 'HK', status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com') === 'unsupported_egress:HK')
check('中国大陆拒绝', shouldBlockEgress({ country: 'CN', status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com') === 'unsupported_egress:CN')
check('stale 结果同样参与判定', shouldBlockEgress({ country: 'HK', status: 'stale' }, 'daily-cloudcode-pa.googleapis.com') === 'unsupported_egress:HK')
check('探测未知 → fail-closed', shouldBlockEgress({ country: '', status: 'unknown' }, 'daily-cloudcode-pa.googleapis.com') === 'unsupported_egress:UNKNOWN')
check('缺 country → fail-closed', shouldBlockEgress({ status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com') === 'unsupported_egress:UNKNOWN')
check('入参缺失 → fail-closed', shouldBlockEgress(undefined, 'daily-cloudcode-pa.googleapis.com') === 'unsupported_egress:UNKNOWN')
check('非合规 host 不参与地理门禁', shouldBlockEgress({ country: 'CN', status: 'fresh' }, 'generativelanguage.googleapis.com') === null)
check('非合规 host 探测失败也不拦', shouldBlockEgress({ status: 'unknown' }, 'oauth2.googleapis.com') === null)
check('自定义国家白名单生效', shouldBlockEgress({ country: 'HK', status: 'fresh' }, 'daily-cloudcode-pa.googleapis.com', { allowedCountries: ['HK'] }) === null)
check('默认国家白名单含 US/JP/SG', ['US', 'JP', 'SG'].every((c) => DEFAULT_ALLOWED_EGRESS_COUNTRIES.includes(c)))
check('默认国家白名单不含 HK/CN', !DEFAULT_ALLOWED_EGRESS_COUNTRIES.includes('HK') && !DEFAULT_ALLOWED_EGRESS_COUNTRIES.includes('CN'))
check('env 国家白名单解析', parseAllowedEgressCountries('us, jp ,sg').join(',') === 'US,JP,SG')
check('env 空串回落默认', parseAllowedEgressCountries('').length === DEFAULT_ALLOWED_EGRESS_COUNTRIES.length)

console.log(`\n结果: ${passed} pass, ${failed} fail`)
process.exit(failed ? 1 : 0)
