/* clipaste site — hero terminal playback + click-to-copy install commands.
   No framework, no build step: this file is served as-is from GitHub Pages. */

(function () {
  'use strict';

  var reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* ── the demo script ──────────────────────────────────────────────
     Each entry is either typed character by character (`type`) or
     printed instantly (`out`), which is how a real session reads:
     you type, the machine answers. `cls` maps to the .t-* colours. */

  var SCRIPT = [
    { out: '« crop a screenshot: Cmd+Shift+4 »', cls: 't-c', pause: 420 },
    { out: '' },
    { type: 'claude', prompt: '$ ', pause: 260 },
    { out: '' },
    { type: 'why is this layout breaking?  ', prompt: '› ', promptCls: 't-g', pause: 120 },
    { out: '^V', cls: 't-k', inline: true, pause: 560 },
    { out: '  ⚠ No image detected in clipboard', cls: 't-err', pause: 1500 },
    { out: '' },
    { out: '« the terminal never had the pixels »', cls: 't-c', pause: 900 },
    { out: '' },
    { type: 'brew install hqhq1025/clipaste/clipaste', prompt: '$ ', pause: 220 },
    { out: '  ==> Installing clipaste 2.4.2 from source', cls: 't-dim', pause: 90 },
    { out: '  ==> clipaste daemon started (9 MB)', cls: 't-dim', pause: 780 },
    { out: '' },
    { type: 'why is this layout breaking?  ', prompt: '› ', promptCls: 't-g', pause: 120 },
    { out: '^V', cls: 't-k', inline: true, pause: 520 },
    { out: '  ▸ [Image #1 attached]  1284×812 png', cls: 't-ok', pause: 2600 }
  ];

  var demo = document.getElementById('demo');
  var note = document.getElementById('demo-note');

  function esc(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  /* Render the whole script instantly. Used for reduced-motion, and as
     the fallback if anything in the animation path throws — a static
     terminal still communicates the before/after. */
  function renderStatic() {
    var html = '';
    SCRIPT.forEach(function (step, i) {
      if (step.type) {
        html += '<span class="' + (step.promptCls || 't-p') + '">' +
          esc(step.prompt || '') + '</span>' + esc(step.type);
        var peek = SCRIPT[i + 1];
        if (!peek || !peek.inline) html += '\n';
      } else {
        html += (step.cls ? '<span class="' + step.cls + '">' + esc(step.out) + '</span>'
                          : esc(step.out)) + '\n';
      }
    });
    demo.innerHTML = html;
  }

  function play() {
    var stepIndex = 0;
    var html = '';

    function cursor() { return '<span class="cursor"></span>'; }
    function paint(extra) { demo.innerHTML = html + (extra || '') + cursor(); }

    function next() {
      if (stepIndex >= SCRIPT.length) {
        // Loop, but leave the success state on screen long enough to read.
        setTimeout(function () {
          html = '';
          stepIndex = 0;
          next();
        }, 1200);
        return;
      }

      var step = SCRIPT[stepIndex++];

      if (step.type) {
        var prompt = '<span class="' + (step.promptCls || 't-p') + '">' +
          esc(step.prompt || '') + '</span>';
        var i = 0;
        var typed = '';
        (function tick() {
          if (i < step.type.length) {
            typed += esc(step.type[i++]);
            paint(prompt + typed);
            // Vary the cadence slightly; perfectly even typing reads as fake.
            setTimeout(tick, 26 + Math.random() * 34);
          } else {
            html += prompt + typed;
            var peek = SCRIPT[stepIndex];
            if (!peek || !peek.inline) html += '\n';
            setTimeout(next, step.pause || 200);
          }
        })();
        return;
      }

      var line = step.cls
        ? '<span class="' + step.cls + '">' + esc(step.out) + '</span>'
        : esc(step.out);
      html += line + '\n';
      paint();
      setTimeout(next, step.pause || 220);
    }

    next();
  }

  if (demo) {
    if (reduceMotion) {
      renderStatic();
      if (note) note.textContent = 'before → after';
    } else {
      try {
        play();
      } catch (e) {
        renderStatic();
      }
    }
  }

  /* ── click-to-copy install commands ──────────────────────────────
     The label swaps in place rather than firing a toast: the feedback
     belongs where the click happened. */

  var COPY_RESET_MS = 1600;

  function flash(btn, text) {
    var label = btn.querySelector('.copy');
    if (!label) return;
    if (btn._resetTimer) clearTimeout(btn._resetTimer);
    var original = btn._originalLabel || (btn._originalLabel = label.textContent);
    label.textContent = text;
    btn.classList.add('copied');
    btn._resetTimer = setTimeout(function () {
      label.textContent = original;
      btn.classList.remove('copied');
    }, COPY_RESET_MS);
  }

  Array.prototype.forEach.call(document.querySelectorAll('.cmd'), function (btn) {
    btn.addEventListener('click', function () {
      var text = btn.getAttribute('data-copy') || '';
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(
          function () { flash(btn, 'copied'); },
          // Clipboard access can be denied (insecure context, permissions).
          // Say so rather than showing a success that did not happen.
          function () { flash(btn, 'select it'); }
        );
      } else {
        flash(btn, 'select it');
      }
    });
  });
})();
