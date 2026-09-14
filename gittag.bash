V=${1:?usage: bash gittag.bash 0.2.0}
V=v${V#v}
git tag -f "$V" && git push -f origin "$V"