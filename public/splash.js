// 启动屏脚本（自 index.html 内联 <script> 外置，满足 CSP script-src 'self'，清单 S-01/Q-00）
document.addEventListener('contextmenu', function (e) {
  e.preventDefault()
})
window.__hideSplash = function () {
  var el = document.getElementById('splash')
  if (el && !el.classList.contains('done')) {
    el.classList.add('done')
    setTimeout(function () {
      if (el.parentNode) el.parentNode.removeChild(el)
    }, 400)
  }
}
// 10 秒绝对兜底
setTimeout(function () {
  if (window.__hideSplash) window.__hideSplash()
}, 10000)
