set -euxo pipefail

mkdir -p work problems toolchains
rm -fr ./work/
mkdir ./work

(cd work && git clone https://github.com/jjs-dev/pps --depth 1)
cp -r work/pps/example-problems/* ./problems
(cd work && git clone https://github.com/jjs-dev/toolchains --depth 1)
cp -r work/toolchains/{gcc,gcc-cpp,python3} ./toolchains