#!/usr/bin/env python3
"""Tell IndexNow search engines (Bing, Yandex, Seznam, Naver and others)
about every URL in sitemap.xml. Run it after each deploy, once the new pages
and the key file are live.

    python3 site/indexnow.py            # submit
    python3 site/indexnow.py --dry-run  # print what would be sent

The key file `<KEY>.txt` must be served at the site root; IndexNow fetches it
to check that the submission comes from the site's owner.
"""

import argparse
import json
import sys
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

HOST = "hatchzip.dev"
KEY = "4114b1f4eb288ada55a869db5d3b852f"
KEY_LOCATION = f"https://{HOST}/{KEY}.txt"
ENDPOINT = "https://api.indexnow.org/indexnow"
SITEMAP = Path(__file__).resolve().parent / "sitemap.xml"
NS = {"sm": "http://www.sitemaps.org/schemas/sitemap/0.9"}


def sitemap_urls() -> list[str]:
    root = ET.parse(SITEMAP).getroot()
    urls = [loc.text.strip() for loc in root.findall("sm:url/sm:loc", NS) if loc.text]
    # IndexNow rejects the whole request if one URL is on another host.
    foreign = [u for u in urls if urllib.parse.urlsplit(u).hostname != HOST]
    if foreign:
        sys.exit(f"sitemap.xml has URLs outside {HOST}: {', '.join(foreign)}")
    return urls


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dry-run", action="store_true", help="print the request and send nothing")
    args = parser.parse_args()

    key_file = SITEMAP.parent / f"{KEY}.txt"
    if not key_file.is_file() or key_file.read_text(encoding="utf-8").strip() != KEY:
        sys.exit(f"{key_file.name} is missing or does not contain the key")

    body = {"host": HOST, "key": KEY, "keyLocation": KEY_LOCATION, "urlList": sitemap_urls()}
    payload = json.dumps(body).encode("utf-8")
    if args.dry_run:
        print(f"POST {ENDPOINT}")
        print(json.dumps(body, indent=2))
        return 0

    request = urllib.request.Request(
        ENDPOINT,
        data=payload,
        method="POST",
        headers={"Content-Type": "application/json; charset=utf-8"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            status = response.status
    except urllib.error.HTTPError as err:
        # 400 bad request, 403 key not valid (key file not live yet?),
        # 422 URLs don't belong to the host, 429 too many requests.
        print(f"IndexNow answered {err.code} {err.reason}", file=sys.stderr)
        print(err.read().decode("utf-8", "replace"), file=sys.stderr)
        return 1
    except urllib.error.URLError as err:
        print(f"Could not reach {ENDPOINT}: {err.reason}", file=sys.stderr)
        return 1

    # 200: received. 202: received, the key is still being checked.
    print(f"IndexNow answered {status} for {len(body['urlList'])} URLs")
    return 0


if __name__ == "__main__":
    sys.exit(main())
