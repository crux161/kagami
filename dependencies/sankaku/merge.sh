#!/bin/bash

set -xe
lipo libsankaku_x86_64.dylib libsankaku_arm64.dylib -output libsankaku_universal.dylib -create

