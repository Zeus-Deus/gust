#!/usr/bin/env python3
"""Drive an isolated Gust + real mpv with Wayland keys; preserve other windows.

Run inside the desktop session after building:
  python3 tests/media-navigation.py --binary target/debug/gust --output /tmp/gust-media-qa
The output directory must be new. Requires wtype, hyprctl, grim, magick, ffmpeg,
mpv and Poppler's pdfinfo/pdftoppm for PDF preview checks.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import socket
import subprocess
import time


def run(*args):
    return subprocess.check_output(args, text=True)


def wait_for(check, label, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError, KeyError, AssertionError):
            pass
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {label}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    qa_lock = open("/tmp/gust-qa.lock", "a")
    try:
        fcntl.flock(qa_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise RuntimeError("Another desktop QA run holds /tmp/gust-qa.lock") from None
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    home = output / "home"
    media = output / "media"
    media.mkdir()
    env = os.environ.copy()
    env["GUST_OMARCHY_STATE"] = env.get("GUST_OMARCHY_STATE", str(Path.home() / ".local/state/omarchy"))
    env["HOME"] = str(home)
    for key, suffix in [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"),
                        ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state")]:
        env[key] = str(home / suffix)
        Path(env[key]).mkdir(parents=True)
    state_file = output / "state.json"
    env["GUST_QA_STATE"] = str(state_file)
    env["QT_QPA_PLATFORM"] = "wayland"
    ipc = output / "mpv.sock"
    config = home / ".config/mpv"
    config.mkdir()
    (config / "mpv.conf").write_text(f"input-ipc-server={ipc}\nvolume=0\n")
    images = ["10-image.png", "20-image.png", "50-image # %\nlast.png"]
    for index, (name, color) in enumerate(zip(images + [".hidden.png"], ["#24516a", "#327267", "#76523d", "#777777"])):
        run("magick", "-size", "1000x650", f"xc:{color}", "-fill", "white",
            "-font", "DejaVu-Sans", "-pointsize", "42", "-gravity", "center",
            "-annotate", "0", f"Gust media navigation: image {index + 1}", str(media / name))
    run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=30", str(media / "30-audio.wav"))
    run("ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=15:duration=3",
        "-pix_fmt", "yuv420p", str(media / "40-video.mp4"))
    run("magick", "-size", "600x800", "xc:white", "xc:gray", str(media / "60-report.pdf"))
    (media / "00-folder.png").mkdir()
    (media / "15-notes.txt").write_text("Skip this document.\n")
    (media / "25-archive.zip").write_bytes(b"Not media")
    (media / "35-program.sh").write_text("#!/bin/sh\nexit 99\n")
    (media / "35-program.sh").chmod(0o755)
    previous = json.loads(run("hyprctl", "activewindow", "-j")).get("address")
    log = (output / "gust.log").open("w")
    app = subprocess.Popen([str(args.binary.resolve()), str(media)], env=env, stdout=log, stderr=log)
    player = None
    checks = []

    def state():
        return json.loads(state_file.read_text())

    def key(name, shift=False):
        pid = player["pid"] if player else app.pid
        focus(f"pid:{pid}")
        wait_for(lambda: json.loads(run("hyprctl", "activewindow", "-j")).get("pid") == pid,
                 "keyboard focus")
        run("wtype", *(["-M", "shift"] if shift else []), "-k", name,
            *(["-m", "shift"] if shift else []))
        time.sleep(0.15)

    def focus(window):
        run("hyprctl", "dispatch", f"hl.dsp.focus({{ window = {json.dumps(window)} }})")

    def command(*args):
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(2)
            connection.connect(str(ipc))
            connection.sendall(json.dumps({"command": list(args), "request_id": 1}).encode() + b"\n")
            with connection.makefile("r") as stream:
                for line in stream:
                    response = json.loads(line)
                    if response.get("request_id") == 1:
                        assert response["error"] == "success", response
                        return response.get("data")

    def expect_file(name):
        wait_for(lambda: command("get_property", "path") == str(media / name), name)
        checks.append(f"Opened {name!r}")

    def expect_playing(name):
        expect_file(name)
        wait_for(lambda: not command("get_property", "pause"), f"{name} is not paused", timeout=3)
        wait_for(lambda: command("get_property", "time-pos") is not None, f"{name} playback position")
        before = command("get_property", "time-pos")
        wait_for(lambda: command("get_property", "time-pos") > before + 0.4,
                 f"{name} playback advances", timeout=3)
        checks.append(f"Playback advances for {name!r}")

    def open_selected():
        nonlocal player
        key("Return")
        def owned_player(client):
            if client.get("class") != "mpv":
                return False
            status = Path(f'/proc/{client["pid"]}/status').read_text()
            return f"PPid:\t{app.pid}\n" in status
        player = wait_for(lambda: [c for c in json.loads(run("hyprctl", "clients", "-j"))
                                  if owned_player(c)], "media window")[0]
        # The window and IPC socket can precede mpv's startup key bindings.
        wait_for(lambda: any(binding.get("key") == "RIGHT"
                             and binding.get("cmd") == "playlist-next weak"
                             and not binding.get("is_weak")
                             for binding in command("get_property", "input-bindings")),
                 "media navigation bindings")
        focus(f'pid:{player["pid"]}')

    def close_player():
        nonlocal player
        key("Escape")
        wait_for(lambda: all(c["pid"] != player["pid"] for c in json.loads(run("hyprctl", "clients", "-j"))), "Escape closes viewer")
        player = None
        focus(f"pid:{app.pid}")

    def screenshot(name):
        pid = player["pid"] if player else app.pid
        window = next(c for c in json.loads(run("hyprctl", "clients", "-j")) if c["pid"] == pid)
        x, y = window["at"]
        width, height = window["size"]
        run("grim", "-g", f"{x},{y} {width}x{height}", str(output / name))

    try:
        wait_for(lambda: state()["model"]["settled"] and state()["model"]["meta_known"] >= 8, "Gust listing")
        wait_for(lambda: any(c["pid"] == app.pid for c in json.loads(run("hyprctl", "clients", "-j"))), "Gust window")
        focus(f"pid:{app.pid}")
        run("wtype", "20-image")
        wait_for(lambda: state()["model"]["cursor_name"] == images[1], "selected middle image")
        open_selected()
        expect_file(images[1])
        zoom = command("get_property", "video-zoom")
        key("Up")
        wait_for(lambda: command("get_property", "video-zoom") > zoom, "Up zooms into opened image")
        expect_file(images[1])
        screenshot("image-zoomed.png")
        key("Down")
        wait_for(lambda: abs(command("get_property", "video-zoom") - zoom) < 0.001,
                 "Down restores opened image zoom")
        expect_file(images[1])
        checks.append("Viewer Up/Down zoom the image without switching files")
        playlist = command("get_property", "playlist")
        assert [p["filename"] for p in playlist] == [str(media / name) for name in
                                                    [images[0], images[1], "30-audio.wav", "40-video.mp4", images[2]]], playlist
        checks.append("Playlist skips documents, directories, programs, archives and hidden files")
        key("Left")
        expect_file(images[0])
        key("Left")
        expect_file(images[0])
        time.sleep(5.5)
        expect_file(images[0])
        key("Right")
        expect_file(images[1])
        screenshot("image.png")
        key("Right")
        expect_file("30-audio.wav")
        key("space")
        wait_for(lambda: command("get_property", "pause"), "Space pauses audio")
        key("space")
        wait_for(lambda: not command("get_property", "pause"), "Space resumes audio")
        before = command("get_property", "time-pos")
        key("Right", shift=True)
        wait_for(lambda: command("get_property", "time-pos") > before + 3, "Shift+Right seeks")
        checks.append("Audio plays, Space pauses/resumes and Shift+Right seeks")
        screenshot("audio.png")
        key("Right")
        expect_file("40-video.mp4")
        zoom = command("get_property", "video-zoom")
        key("Up")
        wait_for(lambda: command("get_property", "video-zoom") > zoom, "Up zooms into opened video")
        expect_file("40-video.mp4")
        key("Down")
        wait_for(lambda: abs(command("get_property", "video-zoom") - zoom) < 0.001,
                 "Down restores opened video zoom")
        checks.append("Viewer Up/Down zoom video without seeking or switching files")
        screenshot("video.png")
        wait_for(lambda: command("get_property", "eof-reached"), "video stays open after ending")
        key("Left")
        expect_playing("30-audio.wav")
        key("Right")
        expect_playing("40-video.mp4")
        wait_for(lambda: command("get_property", "eof-reached"), "video stays open after ending again")
        key("Right")
        expect_file(images[2])
        key("Right")
        expect_file(images[2])
        key("Left")
        expect_playing("40-video.mp4")
        screenshot("post-eof-video.png")
        close_player()

        def expect_preview(name):
            shown_name = name.replace("\n", "<U+000A>")
            wait_for(lambda: state()["quickLookOpen"] and state()["quickLookName"] == shown_name,
                     f"Quick Look: {name!r}")
            assert state()["model"]["cursor_name"] == shown_name
            assert state()["model"]["location"] == str(media)

        def check_preview_zoom(name):
            wait_for(lambda: state()["quickLookImage"] is not None, "preview image to zoom")
            before = state()["quickLookImage"]
            key("Up")
            wait_for(lambda: state()["quickLookImage"]["zoom"] > before["zoom"] and
                     state()["quickLookImage"]["drawn"][0] > before["drawn"][0], "Up enlarges preview")
            expect_preview(name)
            screenshot("quicklook-zoomed.png")
            key("Down")
            wait_for(lambda: abs(state()["quickLookImage"]["zoom"] - before["zoom"]) < 0.001,
                     "Down restores preview zoom")
            expect_preview(name)

        key("space")
        expect_preview(images[1])
        key("Left")
        expect_preview(images[0])
        key("Left")
        expect_preview(images[0])
        key("Right")
        expect_preview(images[1])
        wait_for(lambda: state()["quickLookImage"] is not None, "preview image")
        screenshot("quicklook-image.png")
        check_preview_zoom(images[1])
        checks.append("Quick Look Up/Down zoom the image without changing the file or cursor")
        key("Right")
        expect_preview("30-audio.wav")
        wait_for(lambda: state()["quickLookAudio"] and
                 json.loads(state()["quickLookAudio"])["state"] == "playing", "preview audio")
        before = json.loads(state()["quickLookAudio"])["pos"]
        key("Right", shift=True)
        wait_for(lambda: json.loads(state()["quickLookAudio"])["pos"] > before + 3,
                 "Shift+Right seeks preview audio")
        key("Up")
        key("Down")
        expect_preview("30-audio.wav")
        key("Right")
        expect_preview("40-video.mp4")
        assert state()["quickLookAudio"] == "", "Previous preview audio must stop"
        key("Right")
        expect_preview(images[2])
        key("Right")
        expect_preview(images[2])
        key("Left")
        expect_preview("40-video.mp4")
        key("Escape")
        checks.append("Quick Look arrows update media and cursor, skip non-media, and stop at either end")
        checks.append("Shift+Right seeks preview audio and changing previews stops playback")

        run("wtype", "15-notes")
        wait_for(lambda: state()["model"]["cursor_name"] == "15-notes.txt", "selected document")
        key("space")
        expect_preview("15-notes.txt")
        key("Left")
        expect_preview(images[0])
        key("Escape")
        checks.append("Document previews can browse neighbouring files without entering folders")

        key("End")
        key("space")
        expect_preview("60-report.pdf")
        wait_for(lambda: state()["quickLookPage"] == [1, 2], "first PDF page")
        check_preview_zoom("60-report.pdf")
        assert state()["quickLookPage"] == [1, 2], "Zoom keeps the current PDF page"
        key("Right")
        wait_for(lambda: state()["quickLookPage"] == [2, 2], "second PDF page")
        expect_preview("60-report.pdf")
        key("Right")
        assert state()["quickLookPage"] == [2, 2], "PDF stops at its last page"
        key("Left")
        wait_for(lambda: state()["quickLookPage"] == [1, 2], "previous PDF page")
        key("Next")
        wait_for(lambda: state()["quickLookPage"] == [2, 2], "Page Down still turns PDF pages")
        screenshot("quicklook-pdf.png")
        key("Escape")
        checks.append("PDF arrows and Page Down still turn pages without changing files")

        key("slash")
        run("wtype", "image")
        wait_for(lambda: state()["model"]["count"] == 3, "filtered images")
        key("Return")
        key("Home")
        open_selected()
        assert len(command("get_property", "playlist")) == 3
        key("Right")
        expect_file(images[1])
        key("Right")
        expect_file(images[2])
        checks.append("A committed folder filter also limits the viewer playlist")
        close_player()
        key("space")
        expect_preview(images[0])
        key("Right")
        expect_preview(images[1])
        key("Right")
        expect_preview(images[2])
        key("Right")
        expect_preview(images[2])
        key("Escape")
        checks.append("Quick Look navigation respects the committed folder filter")
        for view in ["grid", "columns"]:
            run("wtype", ":")
            run("wtype", f"View: {view.title()}")
            key("Return")
            wait_for(lambda: state()["view"] == view, f"{view} view")
            key("Home")
            key("space")
            expect_preview(images[0])
            key("Right")
            expect_preview(images[1])
            key("Left")
            expect_preview(images[0])
            key("Left")
            expect_preview(images[0])
            check_preview_zoom(images[0])
            screenshot(f"quicklook-{view}.png")
            key("Escape")
            checks.append(f"Preview Left/Right browse and Up/Down zoom in {view} view")
        (output / "results.json").write_text(json.dumps({"passed": checks}, indent=2) + "\n")
        print(f"PASS: {len(checks)} checks; screenshots and results in {output}")
    except Exception:
        active = json.loads(run("hyprctl", "activewindow", "-j"))
        failure = {"passed": checks, "active": {key: active.get(key) for key in ["pid", "class"]},
                   "expectedPlayer": player, "mpv": {}}
        for prop in ["path", "playlist-pos", "pause", "eof-reached", "input-bindings"]:
            try:
                failure["mpv"][prop] = command("get_property", prop)
            except (OSError, AssertionError):
                pass
        (output / "failure.json").write_text(json.dumps(failure, indent=2) + "\n")
        screenshot("failure.png")
        raise
    finally:
        try:
            command("quit")
        except (OSError, AssertionError):
            pass
        app.terminate()
        app.wait(timeout=10)
        log.close()
        if previous:
            focus(f"address:{previous}")
        qa_lock.close()


if __name__ == "__main__":
    main()
