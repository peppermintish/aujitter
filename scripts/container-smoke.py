import json
import time
from urllib.request import Request, urlopen
from urllib.error import URLError
for attempt in range(90):
    try:
        request = Request("http://127.0.0.1:9876/api/dashboard", headers={"Authorization": "Bearer ci-only-unique-test-token-32-characters"})
        with urlopen(request, timeout=3) as response:
            assert json.load(response)["app"] == "aujitter"
        print("Container service is reachable")
        break
    except (URLError, OSError):
        time.sleep(1)
else:
    raise SystemExit("Container did not become healthy")
