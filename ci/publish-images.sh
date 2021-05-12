set -euxo pipefail

# GENERATED FILE DO NOT EDIT
if [ "$GITHUB_REF" = "refs/heads/master" ]
then
  TAG="latest"
elif [ "$GITHUB_REF" = "refs/heads/trying" ]
then
  TAG="dev"
elif [ "$GITHUB_REF" = "refs/heads/staging" ]
then
  exit 0
else
  echo "unknown GITHUB_REF: $GITHUB_REF"
  exit 1
fi
echo $GITHUB_TOKEN | docker login ghcr.io -u $GITHUB_ACTOR --password-stdin
docker tag judge ghcr.io/jjs-dev/judge:$TAG
docker push ghcr.io/jjs-dev/judge:$TAG