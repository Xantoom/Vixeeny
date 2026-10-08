#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the FFmpeg that Vixeeny ships: the same sources and libraries as the BtbN GPL build
# (its Docker image holds x264, x265, SVT-AV1, libvpx, dav1d, opus, the NVENC, AMF and oneVPL
# headers, all cross-compiled with MinGW), but only the parts Vixeeny uses: ≈ 30 MB of DLLs
# instead of ≈ 180 MB.
#
#   packaging/ffmpeg/build.sh <out.zip>
#
# Needs Docker. The image and the FFmpeg commit are pinned below; `.github/workflows/ffmpeg.yml`
# runs this (cached until this script changes) and hands the zip to the Windows jobs.
set -euo pipefail

IMAGE="ghcr.io/btbn/ffmpeg-builds/win64-gpl-shared-9.0@sha256:e6738cf07c5607353552e8df94e6293604f2af5307898df7ae8c93e2e8e90645"
# release/9.0, the commit of the BtbN build Vixeeny used before (n9.0.2-22-g46d8f462ee).
export FFMPEG_COMMIT="46d8f462ee"

OUT="$(realpath -m "${1:?usage: build.sh <out.zip>}")"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# What Vixeeny asks FFmpeg for (crates/vixeeny-encode: codecs/registry.toml, audio.rs, denoise.rs,
# thumbnail.rs, recorder.rs, and the decoders its tests read the recordings back with).
export ENCODERS="libx264,libx265,libsvtav1,libvpx_vp9,h264_nvenc,hevc_nvenc,av1_nvenc,h264_amf,hevc_amf,av1_amf,h264_qsv,hevc_qsv,av1_qsv,vp9_qsv,aac,aac_mf,libopus,flac,pcm_s16le,pcm_s24le"
export DECODERS="h264,hevc,av1,libdav1d,vp9,aac,opus,libopus,flac,pcm_s16le,pcm_s24le"
export MUXERS="mp4,mov,matroska,webm"
export DEMUXERS="mov,matroska"
export PARSERS="h264,hevc,av1,vp9,aac,opus,flac"
export FILTERS="abuffer,abuffersink,afftdn,aformat,aresample,anull,buffer,buffersink,format,null"

cat > "$WORK/build.sh" <<'SCRIPT'
set -xe
cd /work
git clone --filter=blob:none --no-checkout https://github.com/FFmpeg/FFmpeg.git ffmpeg
cd ffmpeg
git checkout -q $FFMPEG_COMMIT
./configure --prefix=/work/prefix --pkg-config-flags="--static" $FFBUILD_TARGET_FLAGS \
    --enable-gpl --enable-version3 --disable-debug --enable-shared --disable-static \
    --disable-w32threads --enable-pthreads --disable-autodetect \
    --disable-everything --disable-programs --disable-doc --disable-avdevice --disable-network \
    --enable-zlib --enable-d3d11va --enable-mediafoundation \
    --enable-ffnvcodec --enable-nvenc --enable-amf --enable-libvpl \
    --enable-libx264 --enable-libx265 --enable-libsvtav1 --enable-libvpx --enable-libdav1d \
    --enable-libopus \
    --enable-encoder=$ENCODERS --enable-decoder=$DECODERS \
    --enable-muxer=$MUXERS --enable-demuxer=$DEMUXERS --enable-parser=$PARSERS \
    --enable-bsfs --enable-filter=$FILTERS --enable-protocol=file \
    --extra-cflags="$FF_CFLAGS" --extra-cxxflags="$FF_CXXFLAGS" --extra-libs="$FF_LIBS" \
    --extra-ldflags="$FF_LDFLAGS" --extra-ldexeflags="$FF_LDEXEFLAGS" \
    --cc="$CC" --cxx="$CXX" --ar="$AR" --ranlib="$RANLIB" --nm="$NM" \
    --extra-version=vixeeny || { tail -50 ffbuild/config.log; exit 1; }
make -j$(nproc)
make install
# The layout of the BtbN archives (their variants/windows-install-shared.sh).
mkdir -p /work/out/bin /work/out/lib/pkgconfig /work/out/include
cp /work/prefix/bin/*.dll /work/out/bin/
cp /work/prefix/bin/*.lib /work/out/lib/ 2>/dev/null || true
cp /work/prefix/lib/*.def /work/prefix/lib/*.dll.a /work/out/lib/
cp /work/prefix/lib/pkgconfig/*.pc /work/out/lib/pkgconfig/
sed -i -e 's|^prefix=.*|prefix=${pcfiledir}/../..|' -e 's|/work/prefix|${prefix}|' \
    -e '/Libs.private:/d' /work/out/lib/pkgconfig/*.pc
cp -r /work/prefix/include/* /work/out/include/
cp /work/ffmpeg/COPYING.GPLv3 /work/out/LICENSE.txt
SCRIPT

docker run --rm -u "$(id -u):$(id -g)" -v "$WORK":/work \
    -e FFMPEG_COMMIT -e ENCODERS -e DECODERS -e MUXERS -e DEMUXERS -e PARSERS -e FILTERS \
    "$IMAGE" bash /work/build.sh
ls -la "$WORK/out/bin" "$WORK/out/lib"
rm -f "$OUT"
mkdir -p "$(dirname "$OUT")"
# One top-level folder, like the BtbN archives (build-native drops it).
mv "$WORK/out" "$WORK/ffmpeg-vixeeny"
(cd "$WORK" && zip -q -9 -r "$OUT" ffmpeg-vixeeny)
sha256sum "$OUT"
