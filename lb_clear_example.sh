#!/usr/bin/env bash

# Configuration
LB_SERVER="https://your-listenbrainz-server.com"  # Change to your server URL
USER_TOKEN="your-user-token-here"                  # Get from /settings/
USERNAME="username"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}Starting deletion of all listens for user: ${USERNAME}${NC}"

# Get total listen count
echo "Fetching listen count..."
LISTEN_COUNT=$(curl -s "${LB_SERVER}/1/user/${USERNAME}/listen-count" \
  | jq -r '.payload.count')

if [ -z "$LISTEN_COUNT" ] || [ "$LISTEN_COUNT" == "null" ]; then
  echo -e "${RED}Failed to fetch listen count. Check your server URL and username.${NC}"
  exit 1
fi

echo -e "${GREEN}Found ${LISTEN_COUNT} listens to delete${NC}"

if [ "$LISTEN_COUNT" -eq 0 ]; then
  echo "No listens to delete. Exiting."
  exit 0
fi

# Ask for confirmation
read -p "Are you sure you want to delete ALL ${LISTEN_COUNT} listens? (yes/no): " CONFIRM
if [ "$CONFIRM" != "yes" ]; then
  echo "Aborted."
  exit 0
fi

# Fetch and delete all listens
DELETED=0
MAX_TS=""
BATCH_SIZE=1000

while true; do
  echo -e "\n${YELLOW}Fetching batch of listens...${NC}"

  # Construct URL with max_ts if we have one
  if [ -z "$MAX_TS" ]; then
    URL="${LB_SERVER}/1/user/${USERNAME}/listens?count=${BATCH_SIZE}"
  else
    URL="${LB_SERVER}/1/user/${USERNAME}/listens?count=${BATCH_SIZE}&max_ts=${MAX_TS}"
  fi

  # Fetch listens
  RESPONSE=$(curl -s "$URL")

  # Extract listens array
  LISTENS=$(echo "$RESPONSE" | jq -c '.payload.listens[]')

  if [ -z "$LISTENS" ]; then
    echo -e "${GREEN}No more listens found. Deletion complete!${NC}"
    break
  fi

  # Process each listen - FIXED: using process substitution instead of pipe
  while IFS= read -r listen; do
    LISTENED_AT=$(echo "$listen" | jq -r '.listened_at')
    RECORDING_MSID=$(echo "$listen" | jq -r '.recording_msid')
    TRACK_NAME=$(echo "$listen" | jq -r '.track_metadata.track_name // "Unknown"')

    # Delete the listen
    DELETE_PAYLOAD=$(jq -n \
      --arg lat "$LISTENED_AT" \
      --arg msid "$RECORDING_MSID" \
      '{listened_at: ($lat | tonumber), recording_msid: $msid}')

    DELETE_RESPONSE=$(curl -s -w "\n%{http_code}" \
      -X POST \
      -H "Authorization: Token ${USER_TOKEN}" \
      -H "Content-Type: application/json" \
      -d "$DELETE_PAYLOAD" \
      "${LB_SERVER}/1/delete-listen")

    HTTP_CODE=$(echo "$DELETE_RESPONSE" | tail -n1)

    if [ "$HTTP_CODE" -eq 200 ]; then
      ((DELETED++))
      echo -e "${GREEN}✓${NC} Deleted listen #${DELETED}: ${TRACK_NAME} (${LISTENED_AT})"
    else
      echo -e "${RED}✗${NC} Failed to delete: ${TRACK_NAME} (HTTP ${HTTP_CODE})"
    fi

    # Rate limiting - be gentle on the API
    sleep 0.1
  done < <(echo "$LISTENS")  # FIXED: Process substitution instead of pipe

  # Get the oldest timestamp from this batch for next iteration
  MAX_TS=$(echo "$RESPONSE" | jq -r '.payload.listens[-1].listened_at')

  echo -e "${YELLOW}Progress: ${DELETED}/${LISTEN_COUNT} deleted${NC}"

  # Small delay between batches
  sleep 1
done

echo -e "\n${GREEN}Deletion process completed!${NC}"
echo -e "${YELLOW}Note: Listens are scheduled for deletion and will disappear from the UI after the next hourly cleanup job runs.${NC}"
echo "Total listens scheduled for deletion: ${DELETED}"
