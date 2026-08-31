// gate-policy 单测：node deploy/cf-gate-worker/gate-policy.test.mjs
import {
  DEFAULT_BLOCKED_COLOS,
  isGoogleHost,
  parseBlockedColos,
  shouldBlockColo,
} from './gate-policy.mjs'

let passed = 0
let failed = 0
function check(name, cond, extra = '') {
  if (cond) { passed++; console.log(`  [pass] ${name}`) }
  else { failed++; console.log(`  [fail] ${name} ${extra}`) }
}

console.log('[1] Google host 判定')
check('oauth2.googleapis.com 命中', isGoogleHost('oauth2.googleapis.com'))
check('cloudcode-pa.googleapis.com 命中', isGoogleHost('cloudcode-pa.googleapis.com'))
check('accounts.google.com 命中', isGoogleHost('accounts.google.com'))
check('www.gstatic.com 命中', isGoogleHost('www.gstatic.com'))
check('大写归一', isGoogleHost('OAuth2.GOOGLEAPIS.COM'))
check('末尾点归一', isGoogleHost('googleapis.com.'))
check('youtube.com 不命中', !isGoogleHost('www.youtube.com'))
check('github.com 不命中', !isGoogleHost('github.com'))
check('notgoogleapis.com 不命中（dot-boundary）', !isGoogleHost('notgoogleapis.com'))
check('googleapis.com.evil.cn 不命中', !isGoogleHost('googleapis.com.evil.cn'))
check('googleapis.com.hk 不命中（无区域别名）', !isGoogleHost('googleapis.com.hk'))
check('空 host 不命中', !isGoogleHost(''))

console.log('[2] colo 门禁判定')
check('HKG + google host → 拒', shouldBlockColo('HKG', 'oauth2.googleapis.com') === 'unsupported_colo:HKG')
check('MFM + google host → 拒', shouldBlockColo('MFM', 'google.com') === 'unsupported_colo:MFM')
check('HKG + youtube → 放行（P0-2 防全量倾泻）', shouldBlockColo('HKG', 'www.youtube.com') === null)
check('HKG + github → 放行', shouldBlockColo('HKG', 'github.com') === null)
check('SIN + google host → 放行', shouldBlockColo('SIN', 'oauth2.googleapis.com') === null)
check('NRT + google host → 放行', shouldBlockColo('NRT', 'oauth2.googleapis.com') === null)
check('colo 缺失 fail-open', shouldBlockColo(undefined, 'oauth2.googleapis.com') === null)
check('colo 空串 fail-open', shouldBlockColo('', 'oauth2.googleapis.com') === null)
check('colo 小写归一', shouldBlockColo('hkg', 'google.com') === 'unsupported_colo:HKG')

console.log('[3] env 覆盖')
check('默认黑名单', DEFAULT_BLOCKED_COLOS.join(',') === 'HKG,MFM')
check('env 覆盖生效', parseBlockedColos('hkg, sin ,nrt').join(',') === 'HKG,SIN,NRT')
check('env 空串回退默认', parseBlockedColos('').join(',') === 'HKG,MFM')
check('env undefined 回退默认', parseBlockedColos(undefined).join(',') === 'HKG,MFM')
check(
  'env 覆盖后 SIN 也拒',
  shouldBlockColo('SIN', 'google.com', parseBlockedColos('SIN')) === 'unsupported_colo:SIN',
)

console.log('[4] reason 与桌面端 denied 解析兼容（engine_tunnel.rs 读取 reason 字段，任意字符串均可）')
const reason = shouldBlockColo('HKG', 'google.com')
check('reason 为非空字符串', typeof reason === 'string' && reason.length > 0)
check('reason 不含敏感信息', !/token|bearer|secret/i.test(reason))

console.log(`\n结果: ${passed} pass, ${failed} fail`)
process.exit(failed ? 1 : 0)
