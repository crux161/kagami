#!/bin/bash

set -xe
lipo libnezumi_x86_64.dylib libnezumi_arm64.dylib -output libnezumi_universal.dylib -create

