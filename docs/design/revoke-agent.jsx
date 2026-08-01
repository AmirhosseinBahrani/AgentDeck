(
    <div className="[font-synthesis:none] flex overflow-clip w-190 h-165 flex-col justify-center font-sans p-10 bg-deck-void antialiased text-sm/4" style={{ backgroundImage: 'radial-gradient(circle farthest-corner at 50% 0% in oklab, oklab(50% -0.100 -0.046 / 10%) 0%, oklab(0% 0 0 / 0%) 60%)' }}>
      <div className="flex flex-col rounded-[14px] overflow-clip [box-shadow:#000000BF_0px_32px_80px_-20px] bg-[#0F1319] border border-solid border-[#FFFFFF1C]">
        <div className="flex flex-col pt-5.5 pb-4.5 gap-2 px-6 border-b border-b-solid border-b-[#FFFFFF12]">
          <div className="flex items-center gap-2.5">
            <div className="tracking-tight inline-block font-[system-ui,sans-serif] font-semibold text-deck-text text-lg/6">
              Revoke Frontend Engineer
            </div>
            <div className="flex items-center h-5 px-2 rounded-[99px] gap-1.5 bg-[#F2B03624]">
              <div className="w-1.25 h-1.25 rounded-[99px] shrink-0 bg-deck-attention" />
              <div className="tracking-[0.06em] inline-block font-[system-ui,sans-serif] font-semibold text-deck-attention text-micro/3">
                BLOCKED · 34m
              </div>
            </div>
          </div>
          <div className="inline-block font-[system-ui,sans-serif] text-deck-dim text-[12.5px]/4.75">
            The agent leaves the roster and stops receiving work. Its history, reports and past diffs stay in the run record.
          </div>
        </div>
        <div className="flex flex-col pt-5 gap-2.75 px-6">
          <div className="tracking-caps inline-block font-[system-ui,sans-serif] font-semibold text-deck-faint text-micro/3">
            WORK IN FLIGHT
          </div>
          <div className="flex items-center h-6.5 gap-3 shrink-0">
            <div className="w-5.5 shrink-0 font-['JetBrains_Mono',system-ui,sans-serif] inline-block text-deck-text text-base/4">
              1
            </div>
            <div className="grow basis-[0%] inline-block font-[system-ui,sans-serif] text-deck-dim text-[12.5px]/4">
              live session, paused mid-task
            </div>
            <div className="font-['JetBrains_Mono',system-ui,sans-serif] shrink-0 inline-block text-deck-faint text-[10.5px]/3.5">
              #4830
            </div>
          </div>
          <div className="flex items-center h-6.5 gap-3 shrink-0">
            <div className="w-5.5 shrink-0 font-['JetBrains_Mono',system-ui,sans-serif] inline-block text-deck-text text-base/4">
              2
            </div>
            <div className="grow basis-[0%] inline-block font-[system-ui,sans-serif] text-deck-dim text-[12.5px]/4">
              assigned tasks — signing client, error states
            </div>
            <div className="font-['JetBrains_Mono',system-ui,sans-serif] shrink-0 inline-block text-deck-faint text-[10.5px]/3.5">
              TASK-124, 127
            </div>
          </div>
          <div className="flex items-center h-6.5 gap-3 shrink-0">
            <div className="w-5.5 shrink-0 font-['JetBrains_Mono',system-ui,sans-serif] inline-block text-deck-attention text-base/4">
              7
            </div>
            <div className="grow basis-[0%] inline-block font-[system-ui,sans-serif] text-deck-dim text-[12.5px]/4">
              uncommitted files in its worktree
            </div>
            <div className="font-['JetBrains_Mono',system-ui,sans-serif] shrink-0 inline-block text-deck-faint text-[10.5px]/3.5">
              agent/signing-ui
            </div>
          </div>
        </div>
        <div className="flex flex-col pt-5.5 gap-2.25 px-6">
          <div className="tracking-caps inline-block font-[system-ui,sans-serif] font-semibold text-deck-faint text-micro/3">
            WHAT HAPPENS TO ITS WORK
          </div>
          <div className="flex items-center py-2.75 px-3.25 rounded-[8px] gap-2.75 bg-[#FFFFFF0B] border border-solid border-[#1AD1D166]">
            <div className="w-3.25 h-3.25 flex items-center justify-center shrink-0 rounded-[99px] [border-width:1.4px] border-solid border-deck-live">
              <div className="rounded-[99px] shrink-0 bg-deck-live size-1.5" />
            </div>
            <div className="flex flex-col grow basis-[0%] gap-0.5">
              <div className="inline-block font-[system-ui,sans-serif] font-medium text-deck-text text-[12.5px]/4">
                Hand the tasks back to the Supervisor
              </div>
              <div className="inline-block font-[system-ui,sans-serif] text-deck-faint text-[11.5px]/3.5">
                It reassigns them next iteration, keeping the dependency graph intact
              </div>
            </div>
          </div>
          <div className="flex items-center py-2.75 px-3.25 rounded-[8px] gap-2.75 border border-solid border-[#FFFFFF12]">
            <div className="w-3.25 h-3.25 shrink-0 rounded-[99px] [border-width:1.4px] border-solid border-[#FFFFFF38]" />
            <div className="flex flex-col grow basis-[0%] gap-0.5">
              <div className="inline-block font-[system-ui,sans-serif] font-medium text-deck-dim text-[12.5px]/4">
                Return them to the backlog, unassigned
              </div>
              <div className="inline-block font-[system-ui,sans-serif] text-deck-faint text-[11.5px]/3.5">
                Nothing restarts until you assign them yourself
              </div>
            </div>
          </div>
        </div>
        <div className="flex items-center pt-4 gap-2.75 px-6">
          <div className="w-3.75 h-3.75 flex items-center justify-center shrink-0 rounded-[4px] bg-[#1AD1D133] border border-solid border-[#1AD1D180]">
            <svg width="9" height="9" viewBox="0 0 10 10" xmlns="http://www.w3.org/2000/svg" style={{ flexShrink: '0' }}>
              <path d="M1.5 5.2l2.4 2.4L8.5 3" fill="none" stroke="oklch(78% 0.130 195)" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </div>
          <div className="grow basis-[0%] inline-block font-[system-ui,sans-serif] text-deck-dim text-[12.5px]/4">
            Keep the branch and worktree so the 7 uncommitted files survive
          </div>
        </div>
        <div className="flex items-center mt-5.5 py-4.5 px-6 gap-4 bg-[#FFFFFF06] border-t border-t-solid border-t-[#FFFFFF12]">
          <div className="grow basis-[0%] inline-block font-[system-ui,sans-serif] text-deck-faint text-[11.5px]/4.25">
            Reversible — you can rehire this role from its profile, with the same brief and permissions.
          </div>
          <div className="flex items-center shrink-0 gap-2.25">
            <div className="flex items-center h-8.25 px-3.75 rounded-[8px]">
              <div className="inline-block font-[system-ui,sans-serif] font-medium text-deck-faint text-[12.5px]/4">
                Cancel
              </div>
            </div>
            <div className="flex items-center h-8.25 px-4 rounded-[8px] bg-[#F75D5929] border border-solid border-[#F75D5980]">
              <div className="inline-block font-[system-ui,sans-serif] font-semibold text-deck-danger text-[12.5px]/4">
                Revoke access
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  )