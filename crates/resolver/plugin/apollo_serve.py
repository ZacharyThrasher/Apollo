# Apollo warm resolver worker: a yt-dlp *extractor plugin* that turns one long-lived yt-dlp
# process into a request/response resolver.
#
# Why: starting `yt-dlp.exe` per track costs several seconds before the network is even touched
# (PyInstaller unpack, Python imports, a fresh Deno run that re-parses the 2.5 MB YouTube player,
# a new TLS connection per request). Keeping one process alive pays that once. The plugin only
# adds a request loop; every extraction is still yt-dlp's own, and the pinned executable is
# unchanged.
#
# Started by the app as:  yt-dlp --ignore-config --plugin-dirs <dir> ... apollo-serve:
#
# Protocol: one JSON object per line on stdin, one JSON reply per line on the *original* stdout.
# (fd 1 is moved to stderr at start so nothing yt-dlp prints can corrupt the reply channel.)
#   {"op":"init","deno":"<path>"}                       -> {"event":"ready", ...}  (sent once)
#   {"op":"cookies","id":N,"netscape":"<jar text>"}    -> {"id":N,"ok":true,"n":<cookies>}
#   {"op":"resolve","id":N,"video_id":"<11 chars>"}     -> {"id":N,"ok":true,"info":{...},
#                                                           "phases":[[label,ms],..],"ms":T,
#                                                           "jar":"<text, only if it changed>"}
#                                                        or {"id":N,"ok":false,"error":"<text>"}
#   {"op":"quit"}                                       -> exits
# Nothing is written to disk: cookies arrive over the pipe and yt-dlp's cache is held in memory.
# Secrets (cookies, signed URLs) leave this process only through the reply channel; phase labels
# are static yt-dlp progress lines with ids stripped.

import io
import json
import os
import re
import sys
import time

from yt_dlp.extractor.common import InfoExtractor

_ID_RE = re.compile(r'^[A-Za-z0-9_-]{11}$')
_PHASE_ID = re.compile(r'\b[A-Za-z0-9_-]{11}: ')
_INFO_KEYS = (
    'id', 'url', 'protocol', 'acodec', 'abr', 'asr', 'duration', 'format_id', 'format_note',
    'ext', 'http_headers', '_has_drm',
)


class _MemCache:
    """yt-dlp's cache, but in memory only: the preprocessed YouTube player (the expensive part
    of the JS challenge) survives between requests and nothing ever touches the disk."""

    enabled = True
    _MAX = 8

    def __init__(self):
        self._d = {}

    def store(self, section, key, data, dtype='json'):
        if len(self._d) >= self._MAX and (section, key) not in self._d:
            self._d.pop(next(iter(self._d)))
        self._d[(section, key)] = data

    def load(self, section, key, dtype='json', default=None, *, min_ver=None):
        return self._d.get((section, key), default)

    def remove(self):
        self._d.clear()


class _Log:
    """Collects yt-dlp's progress lines as (label, elapsed ms) phases."""

    def __init__(self):
        self.reset()

    def reset(self):
        self.t0 = time.monotonic()
        self.phases = []
        self.last_error = ''

    def _ms(self):
        return int((time.monotonic() - self.t0) * 1000)

    def debug(self, msg):
        # yt-dlp routes its progress lines through debug() when a logger is installed.
        if not msg.startswith('[debug]'):
            self.info(msg)

    def info(self, msg):
        if msg.startswith('[') and 'Extracting URL' not in msg:
            label = _PHASE_ID.sub('', msg)[:70]
            self.phases.append([label, self._ms()])

    def warning(self, msg):
        pass

    def error(self, msg):
        self.last_error = msg


def _enable_player_cache():
    try:
        from yt_dlp.extractor.youtube.jsc._builtin import ejs
        ejs.EJSBaseJCP._ENABLE_PREPROCESSED_PLAYER_CACHE = True
        return True
    except Exception:
        return False


def _client_of(url):
    """The YouTube client that produced a stream URL (its `c=` query value, e.g. WEB_REMIX)."""
    try:
        from urllib.parse import parse_qs, urlsplit
        return (parse_qs(urlsplit(url).query).get('c') or [''])[0][:24]
    except Exception:
        return ''


def _jar_text(ydl):
    buf = io.StringIO()
    ydl.cookiejar.save(buf, ignore_discard=True, ignore_expires=True)
    return buf.getvalue()


def _make_ydl(deno, log, extractor_args=None):
    import yt_dlp
    params = dict(
        logger=log,
        noplaylist=True,
        format='bestaudio',
        skip_download=True,
        ignoreconfig=True,
        cachedir=False,
        remote_components=[],
        plugin_dirs=[],
        socket_timeout=15,
        extractor_retries=0,
        color={'stdout': 'never', 'stderr': 'never'},
    )
    if deno:
        params['js_runtimes'] = {'deno': {'path': deno}}
    if isinstance(extractor_args, dict):
        params['extractor_args'] = extractor_args
    ydl = yt_dlp.YoutubeDL(params)
    ydl.cache = _MemCache()
    return ydl


def _reply(out, obj):
    out.write(json.dumps(obj, separators=(',', ':')) + '\n')
    out.flush()


def serve():
    started = time.monotonic()
    # Keep a private handle on the real stdout, then point fd 1 at stderr.
    out = os.fdopen(os.dup(1), 'w', encoding='utf-8', newline='\n')
    os.dup2(2, 1)
    sys.stdout = sys.stderr
    player_cache = _enable_player_cache()
    log = _Log()
    ydl = None
    jar_seen = ''
    for raw in sys.stdin.buffer:
        try:
            req = json.loads(raw)
        except ValueError:
            continue
        op = req.get('op')
        rid = req.get('id')
        if op == 'quit':
            break
        try:
            if op == 'init':
                ydl = _make_ydl(req.get('deno'), log, req.get('extractor_args'))
                jar_seen = _jar_text(ydl)
                _reply(out, {
                    'event': 'ready',
                    'startup_ms': int((time.monotonic() - started) * 1000),
                    'player_cache': player_cache,
                })
            elif op == 'cookies':
                ydl.cookiejar.clear()
                ydl.cookiejar.load(io.StringIO(req['netscape']), ignore_discard=True,
                                   ignore_expires=True)
                jar_seen = _jar_text(ydl)
                _reply(out, {'id': rid, 'ok': True, 'n': len(ydl.cookiejar)})
            elif op == 'resolve':
                vid = req.get('video_id', '')
                if not _ID_RE.match(vid):
                    _reply(out, {'id': rid, 'ok': False, 'error': 'bad video id'})
                    continue
                if isinstance(req.get('extractor_args'), dict):
                    # Per request, so a click can use faster settings than background work.
                    ydl.params['extractor_args'] = req['extractor_args']
                log.reset()
                t0 = time.monotonic()
                try:
                    info = ydl.extract_info('https://music.youtube.com/watch?v=' + vid,
                                            download=False)
                except Exception as exc:  # yt-dlp raises DownloadError with its message
                    _reply(out, {'id': rid, 'ok': False, 'error': str(exc)[:2000],
                                 'ms': int((time.monotonic() - t0) * 1000),
                                 'phases': log.phases})
                    continue
                reply = {
                    'id': rid, 'ok': True,
                    'client': _client_of(info.get('url')),
                    'info': {k: info[k] for k in _INFO_KEYS if k in info},
                    'phases': log.phases,
                    'ms': int((time.monotonic() - t0) * 1000),
                }
                jar = _jar_text(ydl)
                if jar != jar_seen:
                    reply['jar'] = jar
                    jar_seen = jar
                _reply(out, reply)
            elif op == 'ping':
                _reply(out, {'id': rid, 'ok': True})
        except Exception as exc:
            _reply(out, {'id': rid, 'ok': False, 'error': 'worker: ' + str(exc)[:500]})
    os._exit(0)


class ApolloServeIE(InfoExtractor):
    IE_NAME = 'apollo-serve'
    _VALID_URL = r'apollo-serve:'

    def _real_extract(self, url):
        serve()
