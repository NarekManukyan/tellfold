// Tellfold site. Loaded in <head> so the page knows before first paint whether scripts and motion are on; the rest
// waits for the DOM. Without this file, or with reduced motion, every section is simply there and nothing moves.
(function () {
  var root = document.documentElement;
  var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  root.classList.add('js');
  if (!still) root.classList.add('motion');

  document.addEventListener('DOMContentLoaded', revealOnScroll);
  document.addEventListener('DOMContentLoaded', holdFilm);
  document.addEventListener('DOMContentLoaded', countVisit);

  // Page views and clicks on links marked data-count (Download, Sponsor) go to Tellfold's own counter
  // (stats/ in the repo), only from the live site. It keeps a number per day, page or link, and where the visit came
  // from (a ?ref= tag or the referring site), and nothing about the visitor: no cookies, no IP address. Open the
  // page with #no-count once to stop counting your own visits in this browser; open it with #count to resume.
  var COUNTER = 'https://tellfold-stats.tellfold.workers.dev/hit';

  function countVisit() {
    if (location.hostname !== 'narekmanukyan.github.io' || navigator.webdriver || !navigator.sendBeacon) return;
    try {
      if (location.hash === '#no-count') localStorage.setItem('tellfold-no-count', '1');
      if (location.hash === '#count') localStorage.removeItem('tellfold-no-count');
      if (localStorage.getItem('tellfold-no-count')) return;
    } catch (e) { /* storage blocked: count as usual */ }

    var from = visitSource();
    hit(location.pathname, from);
    document.addEventListener('click', function (event) {
      var link = event.target.closest && event.target.closest('a[data-count]');
      if (link) hit(link.getAttribute('data-count'), from);
    });
  }

  function visitSource() {
    var tag = new URLSearchParams(location.search);
    tag = tag.get('ref') || tag.get('utm_source');
    if (tag) return tag;
    try {
      var host = new URL(document.referrer).hostname.replace(/^www\./, '');
      if (host && host !== location.hostname) return host;
    } catch (e) { /* no referrer */ }
    return 'direct';
  }

  function hit(name, from) {
    navigator.sendBeacon(COUNTER + '?n=' + encodeURIComponent(name) + '&s=' + encodeURIComponent(from));
  }

  // The film loop autoplays muted. For people who ask for less motion it never starts (CSS shows its poster instead).
  function holdFilm() {
    // Phones play the square cut (picked by <source media>); give them the matching poster too.
    if (window.matchMedia('(max-width: 759px)').matches) {
      document.querySelectorAll('video[data-poster-narrow]').forEach(function (v) {
        v.poster = v.getAttribute('data-poster-narrow');
      });
    }
    if (!still) return;
    document.querySelectorAll('video[autoplay]').forEach(function (v) {
      v.removeAttribute('autoplay');
      v.preload = 'none';
      v.pause();
    });
  }

  function revealOnScroll() {
    var items = document.querySelectorAll('.reveal');
    if (!root.classList.contains('motion') || !('IntersectionObserver' in window)) {
      items.forEach(function (el) { el.classList.add('in'); });
      return;
    }
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        entry.target.classList.add('in');
        io.unobserve(entry.target);
      });
    }, { rootMargin: '0px 0px -8% 0px', threshold: 0.12 });
    items.forEach(function (el) { io.observe(el); });
  }
})();
